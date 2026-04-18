mod compaction;
mod convert;
mod observer;
mod tool;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentkit::compaction::{CompactionConfig, CompactionPipeline, SummarizeOlderStrategy};
use agentkit::core::{CancellationController, Item, ItemKind};
use agentkit::loop_::{
    Agent, LoopInterrupt, LoopStep, PromptCacheRequest, PromptCacheRetention, SessionConfig,
};
use agentkit::provider_openrouter::{OpenRouterAdapter, OpenRouterConfig};
use agentkit::tools::{CompositePermissionChecker, PermissionDecision};
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::anthropic::client::LlmClient2;
use crate::anthropic::types::{Message, Usage};
use crate::config::KernelConfig;

use self::compaction::{
    AnyOfTrigger, KernelCompactionBackend, LargeToolResultTrigger, PersistSnapshotStrategy,
    PreservePinnedStrategy, TokenBudgetTrigger, TruncateToolResultsStrategy,
};
use self::convert::{build_message_meta, items_to_messages, messages_to_items_with_meta};
use self::observer::{finish_reason_str, AgentRunState, TauriEventObserver};
use self::tool::build_registry;

/// How many transcript items to keep verbatim after a deep compaction. Older
/// removable items are sent to the backend for summarisation.
const COMPACTION_KEEP_LAST: usize = 24;

/// Reserved tokens (system prompt + response budget) subtracted from the
/// model context window when computing the deep-compaction threshold.
const RESERVED_SYSTEM_TOKENS: usize = 2_000;
const RESERVED_RESPONSE_TOKENS: usize = 4_096;

#[derive(Clone, Serialize)]
struct LlmDone {
    stop_reason: String,
    full_text: String,
}

#[derive(Clone, Serialize)]
struct LlmError {
    message: String,
}

/// Run the agentic loop via agentkit.
///
/// `pinned_data` maps the message index in `messages` to the existing
/// `context_snippet` (if any) for pinned messages. `ordinals` is the list
/// of DB ordinals aligned with `messages`. Both are stamped onto the
/// agentkit `Item.metadata` so compaction strategies can identify pinned
/// messages and the backend can persist snapshots tied to real DB rows.
///
/// Returns `(full_text, accumulated_usage)`. Newly produced messages are
/// appended to `messages` and persisted to the DB before returning.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_loop(
    system_prompt: &str,
    model: &str,
    allowed_tools: &[String],
    project_path: &str,
    app: &AppHandle,
    messages: &mut Vec<Message>,
    pinned_data: &HashMap<usize, Option<String>>,
    ordinals: &[Option<i64>],
    session_id: &str,
    pool: &SqlitePool,
    cancelled: Arc<AtomicBool>,
    config: &KernelConfig,
    context_window: usize,
) -> Result<(String, Usage), String> {
    info!(model, context_window, tools = allowed_tools.len(), "agentkit loop starting");
    let project = Path::new(project_path);

    let api_key = resolve_openrouter_api_key(config)?;
    let base_url = resolve_openrouter_base_url(config);

    let normalized_model = normalize_model(model);
    let mut or_config = OpenRouterConfig::new(api_key, normalized_model)
        .with_app_name("kernel")
        .with_max_completion_tokens(16_384);
    if let Some(url) = base_url {
        or_config = or_config.with_base_url(url);
    }
    let adapter = OpenRouterAdapter::new(or_config).map_err(|e| e.to_string())?;

    let registry = build_registry(allowed_tools, project, app.clone(), pool, session_id).await;

    let permissions = CompositePermissionChecker::new(PermissionDecision::Allow);

    let state = Arc::new(Mutex::new(AgentRunState::default()));
    let observer = TauriEventObserver {
        app: app.clone(),
        session_id: session_id.to_string(),
        context_window,
        state: Arc::clone(&state),
    };

    let cancel_controller = CancellationController::new();
    let cancel_handle = cancel_controller.handle();

    // ---- Build compaction pipeline ----
    let compaction_config = build_compaction_config(
        config,
        context_window,
        pool.clone(),
        session_id.to_string(),
    )?;

    let agent = Agent::builder()
        .model(adapter)
        .tools(registry)
        .permissions(permissions)
        .observer(observer)
        .cancellation(cancel_handle)
        .compaction(compaction_config)
        .build()
        .map_err(|e| e.to_string())?;

    let mut driver = agent
        .start(
            SessionConfig::new(session_id).with_cache(
                PromptCacheRequest::automatic().with_retention(PromptCacheRetention::Short),
            ),
        )
        .await
        .map_err(|e| e.to_string())?;

    // Submit initial transcript: system prompt + existing messages with
    // pin/ordinal metadata stamped in.
    let mut input: Vec<Item> = Vec::new();
    if !system_prompt.is_empty() {
        input.push(Item::text(ItemKind::System, system_prompt));
    }
    let meta = build_message_meta(messages.len(), pinned_data, ordinals);
    input.extend(messages_to_items_with_meta(messages, &meta));
    driver.submit_input(input).map_err(|e| e.to_string())?;

    // Watcher task: forwards the external AtomicBool cancel flag into agentkit's controller.
    let controller_for_watch = cancel_controller.clone();
    let cancel_for_watch = Arc::clone(&cancelled);
    let watch_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if cancel_for_watch.load(Ordering::SeqCst) {
                controller_for_watch.interrupt();
                break;
            }
        }
    });

    let mut final_step: Option<LoopStep> = None;
    let mut run_error: Option<String> = None;

    const MAX_DRIVER_STEPS: usize = 200;
    for _ in 0..MAX_DRIVER_STEPS {
        if cancelled.load(Ordering::SeqCst) {
            cancel_controller.interrupt();
        }
        match driver.next().await {
            Ok(LoopStep::Finished(result)) => {
                final_step = Some(LoopStep::Finished(result));
                break;
            }
            Ok(LoopStep::Interrupt(LoopInterrupt::AwaitingInput(req))) => {
                warn!(reason = %req.reason, "loop requested more input — ending");
                final_step = Some(LoopStep::Interrupt(LoopInterrupt::AwaitingInput(req)));
                break;
            }
            Ok(LoopStep::Interrupt(other)) => {
                warn!(?other, "unexpected loop interrupt — ending");
                final_step = Some(LoopStep::Interrupt(other));
                break;
            }
            Err(e) => {
                run_error = Some(e.to_string());
                break;
            }
        }
    }

    watch_handle.abort();
    let _ = watch_handle.await;

    // Persist any new items the observer captured per-turn. Using observer
    // state instead of slicing the transcript is required because deep
    // compaction can shrink the transcript below its initial length, which
    // would otherwise make new items unobservable.
    let new_items: Vec<Item> = {
        let s = state.lock().map_err(|_| "observer state poisoned".to_string())?;
        s.run_items.clone()
    };
    let new_messages = items_to_messages(&new_items);
    for msg in &new_messages {
        let _ = append_message(pool, session_id, msg).await;
    }
    messages.extend(new_messages);

    let (full_text, usage, finish_reason) = {
        let s = state.lock().map_err(|_| "observer state poisoned".to_string())?;
        (s.full_text.clone(), s.usage.clone(), s.finish_reason.clone())
    };

    if let Some(err) = run_error {
        if is_model_unsupported_error(&err) {
            return Err(err);
        }
        let _ = app.emit("llm-error", LlmError { message: err.clone() });
        let _ = app.emit(
            "llm-done",
            LlmDone {
                stop_reason: "error".into(),
                full_text: full_text.clone(),
            },
        );
        return Ok((full_text, usage));
    }

    let stop_reason: String = match final_step {
        Some(LoopStep::Finished(result)) => finish_reason_str(&result.finish_reason).to_string(),
        Some(LoopStep::Interrupt(_)) => {
            if cancelled.load(Ordering::SeqCst) {
                "cancelled".to_string()
            } else {
                "interrupted".to_string()
            }
        }
        None => "max_turns".to_string(),
    };

    let _ = app.emit(
        "llm-done",
        LlmDone {
            stop_reason: if stop_reason.is_empty() {
                finish_reason.clone()
            } else {
                stop_reason
            },
            full_text: full_text.clone(),
        },
    );

    Ok((full_text, usage))
}

/// Construct the agentkit `CompactionConfig` for a run.
///
/// Pipeline layout (outermost first):
///   PersistSnapshotStrategy
///     └─ CompactionPipeline
///         ├─ TruncateToolResultsStrategy           (light, in-place)
///         └─ PreservePinnedStrategy                (skips pinned items)
///             └─ SummarizeOlderStrategy            (uses backend for summary)
///
/// Trigger fires when *either* a tool result exceeds the size threshold
/// *or* the estimated transcript tokens cross `deep_trigger_pct`.
fn build_compaction_config(
    config: &KernelConfig,
    context_window: usize,
    pool: SqlitePool,
    session_id: String,
) -> Result<CompactionConfig, String> {
    // Backend client: a separate LlmClient2 used solely for compaction calls.
    let client = LlmClient2::from_config(&config.models, "openrouter")
        .or_else(|_| LlmClient2::from_env())
        .map_err(|e| format!("compaction client: {e}"))?;

    let target_tokens = ((context_window as f32)
        * (config.compaction.deep_target_pct.clamp(0.0, 100.0) / 100.0))
        as usize;
    let backend = KernelCompactionBackend::new(
        Arc::new(client),
        config.models.compactor.clone(),
        pool.clone(),
        session_id.clone(),
        target_tokens.max(1024),
    );

    let mut inner = CompactionPipeline::new();
    if config.compaction.light_every_turn {
        inner = inner.with_strategy(TruncateToolResultsStrategy::default());
    }
    inner = inner.with_strategy(PreservePinnedStrategy::wrap(
        SummarizeOlderStrategy::new(COMPACTION_KEEP_LAST)
            .preserve_kind(ItemKind::System)
            .preserve_kind(ItemKind::Context),
    ));

    let outer = PersistSnapshotStrategy::wrap(inner, pool, session_id);

    let trigger = AnyOfTrigger::new()
        .with(LargeToolResultTrigger { max_chars: 2000 })
        .with(TokenBudgetTrigger {
            context_window,
            trigger_pct: config.compaction.deep_trigger_pct,
            reserved_tokens: RESERVED_SYSTEM_TOKENS + RESERVED_RESPONSE_TOKENS,
        });

    Ok(CompactionConfig::new(trigger, outer).with_backend(backend))
}

pub fn is_model_unsupported_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("model not found")
        || lower.contains("unsupported model")
        || lower.contains("model is not available")
        || lower.contains("no endpoints found for model")
        || lower.contains("invalid model")
}

fn normalize_model(model: &str) -> String {
    if model.contains('/') {
        model.to_string()
    } else {
        format!("anthropic/{model}")
    }
}

fn resolve_openrouter_api_key(config: &KernelConfig) -> Result<String, String> {
    let env_var = config
        .models
        .providers
        .get("openrouter")
        .and_then(|p| p.api_key_env.clone())
        .unwrap_or_else(|| "OPENROUTER_API_KEY".to_string());
    std::env::var(&env_var).or_else(|_| std::env::var("OPENROUTER_API_KEY")).map_err(|_| {
        format!(
            "OpenRouter API key not found. Set ${env_var} or $OPENROUTER_API_KEY. \
             (Native Anthropic API is not yet supported by the agentkit adapter.)"
        )
    })
}

fn resolve_openrouter_base_url(config: &KernelConfig) -> Option<String> {
    config
        .models
        .providers
        .get("openrouter")
        .and_then(|p| p.base_url.clone())
        .map(|u| {
            if u.ends_with("/chat/completions") {
                u
            } else if u.ends_with('/') {
                format!("{u}v1/chat/completions")
            } else {
                format!("{u}/v1/chat/completions")
            }
        })
}

async fn append_message(pool: &SqlitePool, session_id: &str, message: &Message) -> Result<i64, String> {
    let role_str = match message.role {
        crate::anthropic::types::Role::User => "user",
        crate::anthropic::types::Role::Assistant => "assistant",
    };
    let content_json = serde_json::to_string(&message.content).map_err(|e| e.to_string())?;
    crate::db::queries::append_conversation_message(pool, session_id, role_str, &content_json, false)
        .await
        .map_err(|e| e.to_string())
}
