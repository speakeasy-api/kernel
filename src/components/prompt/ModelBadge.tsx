interface ModelBadgeProps {
  model: string;
}

function abbreviateModel(model: string): string {
  const match = model.match(/claude-(\w+)-(\d+)-(\d+)/);
  if (match) return `${match[1]} ${match[2]}.${match[3]}`;
  const matchLong = model.match(/claude-(\w+)-(\d+)/);
  if (matchLong) return `${matchLong[1]} ${matchLong[2]}`;
  return model;
}

export function ModelBadge({ model }: ModelBadgeProps) {
  return (
    <span className="inline-flex items-center gap-1.5 text-[11px] font-mono text-text-ghost tracking-tight">
      <span
        className="h-1.5 w-1.5 rounded-full animate-pulse-subtle"
        style={{ backgroundColor: `var(--mode-tint)` }}
      />
      {abbreviateModel(model)}
    </span>
  );
}
