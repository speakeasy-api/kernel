/**
 * Derive a deterministic HSL hue from any mode name.
 * Known modes get hand-picked hues for aesthetics;
 * unknown/custom modes get a stable hash-derived hue.
 */

const KNOWN_HUES: Record<string, number> = {
  auto: 220,      // neutral blue
  general: 265,   // violet
  plan: 210,      // blue
  code: 160,      // teal
  research: 35,   // amber
  review: 330,    // rose
  debug: 15,      // red-orange
};

function hashHue(name: string): number {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  return ((hash % 360) + 360) % 360;
}

export function getModeTint(modeName: string) {
  const hue = KNOWN_HUES[modeName] ?? hashHue(modeName);
  return {
    hue,
    // CSS custom property values — set on the container
    vars: {
      "--mode-hue": `${hue}`,
      "--mode-tint": `hsl(${hue} 50% 60%)`,
      "--mode-tint-dim": `hsl(${hue} 40% 45%)`,
      "--mode-tint-subtle": `hsla(${hue}, 50%, 50%, 0.06)`,
      "--mode-tint-glow": `hsla(${hue}, 60%, 50%, 0.12)`,
      "--mode-tint-muted": `hsla(${hue}, 30%, 60%, 0.15)`,
    } as Record<string, string>,
  };
}
