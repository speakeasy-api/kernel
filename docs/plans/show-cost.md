## Refined Feature Plan: Dollar Cost in Context Window Panel

### Confirmed Architecture Facts

- `KernelConfig.costs.warn_at_usd` and `hard_limit_usd` are the **session-level** USD thresholds (defaults: $5 warn, $20 hard-limit). The frontend `types.ts` already models them correctly.
- `config: KernelConfig | null` is already a prop on `PromptWindow` — so cost thresholds can reach `ContextRing` without any new data-fetching.
- The `llm-usage` event is emitted by the backend with `{ input_tokens, output_tokens, cost_usd }` but **lacks a `session_id` filter field** — this needs a small backend fix.
- The existing arc color pattern (`mode-tint → amber → red`) uses **ratio-based** thresholds. For cost we will use **absolute USD thresholds** from config instead of percentages. The color stops will mirror the same three-level palette.
- `ContextRing` currently has 4 props: `used`, `total`, `items`, `sessionId`. We will add 2 more: `sessionCost` and `costThresholds`.

---

### Complete Change Set

#### **Change 1 — Backend: add `session_id` to `LlmUsage` event**

**File:** `src-tauri/src/prompt_router/commands.rs`

The `LlmUsage` struct (line 223) and its emit site (line 849) need `session_id` added so the frontend can filter events to the active session. The `session_id` is available in the surrounding function scope at the emit site.

```
// Before
struct LlmUsage {
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

// After
struct LlmUsage {
    session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

```

At the emit site, populate `session_id: sid.to_string()` (the variable name `sid` for the session UUID is already in scope at that location in the function).

---

#### **Change 2 — Frontend command wrapper**

**File:** `src/lib/commands.ts`

Add a single new export. The Tauri command is already registered:

```
export function getSessionCost(sessionId: string): Promise<number> {
  return invoke<number>("get_session_cost", { sessionId });
}

```

---

#### **Change 3 — Hook: track `sessionCost` in `useLlmStream`**

**File:** `src/hooks/useLlmStream.ts`

Three additions inside the existing hook:

**3a.** New interface at the top of the file alongside the other payload interfaces:

```
interface LlmUsageEvent {
  session_id: string;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

```

**3b.** New state variable alongside `contextUsage`:

```
const [sessionCost, setSessionCost] = useState<number | null>(null);

```

**3c.** In the existing bootstrap `useEffect` that calls `getConversationHistory`, add a parallel call to `getSessionCost(sessionId)` and set state. This gives the correct DB-authoritative starting value on load or session switch. If the call fails (e.g. no tasks yet), default to `0`:

```
getSessionCost(sessionId)
  .then(setSessionCost)
  .catch(() => setSessionCost(0));

```

**3d.** Add a new event listener in the existing unlisten block, after the `context-usage` listener (`u7`), so it follows the established `u8`numbering pattern:

```
const u8 = await listen<LlmUsageEvent>("llm-usage", (e) => {
  if (e.payload.session_id !== sessionId) return;
  setSessionCost((prev) => (prev ?? 0) + e.payload.cost_usd);
});
if (cancelled) { u8(); return; }
unlistens.push(u8);

```

**3e.** Add `sessionCost` to the return value:

```
return { items, phase, resolvedMode, error, contextUsage, sessionCost, submit, cancel };

```

> **Edge case:** On session switch, the `useEffect` cleanup calls all `unlistens` and resets state. `sessionCost` will be reset to `null`then re-populated by the bootstrap fetch — this is correct.

---

#### **Change 4 — Thread props through `PromptWindow`**

**File:** `src/components/prompt/PromptWindow.tsx`

**4a.** Destructure `sessionCost` from the hook return:

```
const { items, phase, resolvedMode, error, contextUsage, sessionCost, submit, cancel } = useLlmStream(session.id);

```

**4b.** At the `<ContextRing>` call site, pass two new props:

```
<ContextRing
  used={usedTokens}
  total={contextWindow}
  items={items}
  sessionId={session.id}
  sessionCost={sessionCost}
  costThresholds={config?.costs ?? null}
/>

```

`config` is already in scope as a prop of `PromptWindow`. Passing `null` when config hasn't loaded is safe — the component will render cost without color thresholds in that case (falls back to neutral style).

---

#### **Change 5 — Render cost in `ContextRing`**

**File:** `src/components/prompt/ContextRing.tsx`

**5a.** Extend `ContextRingProps`:

```
interface ContextRingProps {
  used: number;
  total: number;
  items: ChatItem[];
  sessionId: string;
  /** Total USD spent in this session, null while loading. */
  sessionCost: number | null;
  /** Cost thresholds from kernel config for color coding. */
  costThresholds: { warn_at_usd: number; hard_limit_usd: number } | null;
}

```

**5b.** Add cost color derivation in the component body, directly below the existing `arcColor` block. This mirrors the same three-level logic:

```
const costColor =
  sessionCost === null || costThresholds === null
    ? "var(--color-text-ghost)"
    : sessionCost < costThresholds.warn_at_usd
      ? "var(--color-text-secondary)"   // neutral — under warn threshold
      : sessionCost < costThresholds.hard_limit_usd
        ? "hsl(38 90% 60%)"             // amber — between warn and hard limit
        : "hsl(4 80% 60%)";             // red — at or over hard limit

```

**5c.** Add a formatting helper (module-level, alongside the existing `countItems` helper at the bottom of the file):

```
function formatCost(usd: number): string {
  if (usd < 0.001) return "$0.00";
  if (usd < 0.01)  return `$${usd.toFixed(3)}`;   // e.g. "$0.004"
  return `$${usd.toFixed(2)}`;                     // e.g. "$0.04", "$12.50"
}

```

**5d.** Add the cost row in the panel header, between the progress bar and the tabs (between lines 153 and 154 in the current file). This keeps the header's visual hierarchy: title → token bar → cost stat → tabs.

```
{/* Session Cost */}
<div className="mt-2 flex items-center justify-between">
  <span className="text-[11px] text-text-ghost">Session cost</span>
  <span
    className="text-[11px] font-mono tabular-nums"
    style={{ color: costColor }}
  >
    {sessionCost === null ? "—" : formatCost(sessionCost)}
  </span>
</div>

```

The `"—"` (em dash) loading placeholder matches the `null` state before the bootstrap fetch resolves and is visually unobtrusive.

---

### Final File Change Summary

#

File

Nature of change

Risk

1

`src-tauri/src/prompt_router/commands.rs`

Add `session_id` field to `LlmUsage` struct + emit site

Very low — additive only, no existing consumers

2

`src/lib/commands.ts`

Add `getSessionCost()` wrapper

Trivial

3

`src/hooks/useLlmStream.ts`

New state, new interface, new listener, bootstrap fetch, expanded return

Low — follows exact existing patterns

4

`src/components/prompt/PromptWindow.tsx`

Destructure `sessionCost`, thread 2 new props

Trivial

5

`src/components/prompt/ContextRing.tsx`

Extend props interface, derive `costColor`, add `formatCost`, add cost row in header

Low — purely additive UI

### Precise Color Behavior

Session cost

Color

Meaning

`null` (loading)

`--color-text-ghost`

Not yet fetched

`< warn_at_usd` (e.g. `< $5`)

`--color-text-secondary`

Healthy / normal

`≥ warn_at_usd`, `< hard_limit_usd` (e.g. `$5–$19.99`)

amber `hsl(38 90% 60%)`

Approaching limit

`≥ hard_limit_usd` (e.g. `≥ $20`)

red `hsl(4 80% 60%)`

At or over hard limit

This exactly mirrors the token arc's three-level palette (mode-tint / amber / red), except the neutral state uses `--color-text-secondary` instead of `--mode-tint` since cost is informational rather than mode-branded. The threshold semantics are now absolute USD values from `config.costs` rather than hardcoded ratios.

write the plan to `plans/`
