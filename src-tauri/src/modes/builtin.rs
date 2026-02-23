use super::types::{combine_tool_sets, Mode, ModeOrigin, FULL_TOOLS, READ_ONLY_TOOLS, WEB_TOOLS};

pub fn builtin_modes() -> Vec<Mode> {
    vec![
        plan_mode(),
        implement_mode(),
        review_mode(),
        debug_mode(),
        research_mode(),
        general_mode(),
    ]
}

fn plan_mode() -> Mode {
    Mode {
        name: "plan".into(),
        description: "Structured decomposition, trade-off analysis, step-by-step reasoning".into(),
        system_prompt: "\
You are in Plan mode. Your role is to help the user think through problems before any code is written or changed.

Focus on structured decomposition: break complex problems into numbered steps with clear dependencies between them. When multiple approaches exist, analyze trade-offs explicitly — list pros, cons, and risks for each option. Do not rush to a solution; ask clarifying questions when the user's intent or constraints are ambiguous.

Consider edge cases and failure modes proactively. Identify assumptions that could invalidate the plan. When estimating scope, flag which parts carry the most uncertainty.

You must never modify files, run shell commands, or take any action that changes project state. Your only tools are reading files, searching with glob, and grepping for context. Use these freely to ground your analysis in the actual codebase rather than guessing at structure or conventions."
            .into(),
        default_model: None,
        allowed_tools: READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect(),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

fn implement_mode() -> Mode {
    Mode {
        name: "implement".into(),
        description: "Code generation, tool-use heavy, minimal explanation".into(),
        system_prompt: "\
You are in Implement mode. Your job is to produce working code, not explanations.

Read existing files aggressively before writing — understand the project's patterns, naming conventions, import style, and error handling approach, then match them. Write complete implementations, never stubs or pseudocode. If a function needs 40 lines, write all 40 lines.

Use tools liberally: search for related code, read tests to understand expected behavior, and examine types before using them. After making changes, run available tests or type checks to verify correctness.

Keep commentary minimal. A one-line summary of what you did is fine; a paragraph explaining why each line exists is not. If the user wants deeper explanation, they will ask. Prioritize shipping correct, consistent code over teaching."
            .into(),
        default_model: None,
        allowed_tools: FULL_TOOLS.iter().map(|s| s.to_string()).collect(),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

fn review_mode() -> Mode {
    Mode {
        name: "review".into(),
        description: "Diff-aware, security and correctness focus, concise feedback".into(),
        system_prompt: "\
You are in Review mode. Your role is to evaluate code for correctness, security, and maintainability.

Use grep to search the codebase and examine what changed. Reference specific files and line numbers in your feedback so the author can act on it quickly.

Prioritize your findings: distinguish blocking issues (bugs, security vulnerabilities, data loss risks, race conditions) from suggestions (style, naming, minor refactors). Flag error handling gaps, unchecked inputs at system boundaries, and resource leaks.

Be concise and actionable. Say what is wrong, why it matters, and what to do about it. Skip praise for code that is simply correct — focus your attention on what needs to change. If everything looks good, say so briefly rather than inventing nitpicks."
            .into(),
        default_model: None,
        allowed_tools: READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect(),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

fn debug_mode() -> Mode {
    Mode {
        name: "debug".into(),
        description: "Hypothesis-driven, log analysis, bisect strategy".into(),
        system_prompt: "\
You are in Debug mode. Your job is to find and fix the root cause of problems, not paper over symptoms.

Start by forming hypotheses from the available evidence — error messages, stack traces, logs, and the user's description. Rank hypotheses by likelihood and test the most probable first. Narrow the search space systematically rather than shotgunning changes.

Reproduce the issue before attempting a fix when possible. Use git history to identify when behavior changed; a bisect strategy is often the fastest path to a root cause. Read surrounding code carefully — bugs frequently live in assumptions about state, ordering, or error propagation.

When you find the fix, explain the root cause clearly: what was wrong, why it happened, and why the fix is correct. Verify that the fix resolves the issue and does not introduce regressions."
            .into(),
        default_model: None,
        allowed_tools: FULL_TOOLS.iter().map(|s| s.to_string()).collect(),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

fn research_mode() -> Mode {
    Mode {
        name: "research".into(),
        description: "Deep reading, summarization, web search".into(),
        system_prompt: "\
You are in Research mode. Your role is to gather information, read deeply, and synthesize clear summaries.

Use web search to find relevant documentation, examples, and discussions. When comparing approaches, present structured pros and cons rather than a single recommendation. Cite your sources — include links so the user can verify and explore further.

Distinguish between established best practices (widely adopted, battle-tested) and opinions or emerging patterns. When documentation is ambiguous or conflicting, say so rather than presenting one interpretation as definitive.

Read project files thoroughly to understand existing context before searching externally. You must never modify project files — your output is information and analysis, not code changes."
            .into(),
        default_model: None,
        allowed_tools: combine_tool_sets(&[READ_ONLY_TOOLS, WEB_TOOLS]),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

fn general_mode() -> Mode {
    Mode {
        name: "general".into(),
        description: "Balanced, conversational".into(),
        system_prompt: "\
You are in General mode. Adapt your approach to match what the user needs — planning, coding, reviewing, researching, or just answering a question.

Use tools as needed but explain what you are doing and why. Provide context and reasoning with your responses so the user can follow your thought process. When a request is ambiguous, ask for clarification rather than guessing.

Balance thoroughness with conciseness: give enough detail to be useful without overwhelming. If a task would benefit from a more specialized mode (deep debugging, focused implementation, thorough review), suggest switching, but handle straightforward requests directly."
            .into(),
        default_model: None,
        allowed_tools: FULL_TOOLS.iter().map(|s| s.to_string()).collect(),
        created_by: ModeOrigin::BuiltIn,
        version: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_modes_returns_six() {
        let modes = builtin_modes();
        assert_eq!(modes.len(), 6);
    }

    #[test]
    fn all_modes_are_builtin_origin() {
        for mode in builtin_modes() {
            assert_eq!(mode.created_by, ModeOrigin::BuiltIn);
        }
    }

    #[test]
    fn all_modes_are_version_one() {
        for mode in builtin_modes() {
            assert_eq!(mode.version, 1);
        }
    }

    #[test]
    fn all_modes_have_no_default_model() {
        for mode in builtin_modes() {
            assert_eq!(mode.default_model, None);
        }
    }

    #[test]
    fn all_system_prompts_are_substantial() {
        for mode in builtin_modes() {
            assert!(
                mode.system_prompt.len() >= 200,
                "mode '{}' system prompt is only {} chars",
                mode.name,
                mode.system_prompt.len()
            );
        }
    }

    #[test]
    fn plan_mode_tools() {
        let mode = plan_mode();
        assert_eq!(mode.name, "plan");
        let expected: Vec<String> = READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn implement_mode_tools() {
        let mode = implement_mode();
        assert_eq!(mode.name, "implement");
        let expected: Vec<String> = FULL_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn review_mode_tools() {
        let mode = review_mode();
        assert_eq!(mode.name, "review");
        let expected: Vec<String> = READ_ONLY_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn debug_mode_tools() {
        let mode = debug_mode();
        assert_eq!(mode.name, "debug");
        let expected: Vec<String> = FULL_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn research_mode_tools() {
        let mode = research_mode();
        assert_eq!(mode.name, "research");
        let expected = combine_tool_sets(&[READ_ONLY_TOOLS, WEB_TOOLS]);
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn general_mode_tools() {
        let mode = general_mode();
        assert_eq!(mode.name, "general");
        let expected: Vec<String> = FULL_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(mode.allowed_tools, expected);
    }

    #[test]
    fn mode_names_are_unique() {
        let modes = builtin_modes();
        let mut names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 6);
    }
}
