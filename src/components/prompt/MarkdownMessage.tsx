import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";

interface MarkdownMessageProps {
  content: string;
  role: "user" | "assistant";
}

const userComponents: Components = {
  p: ({ children }) => <p className="mb-2 last:mb-0">{children}</p>,
  strong: ({ children }) => <strong className="font-semibold text-text-primary">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  code: ({ children, className }) => {
    const isBlock = className?.startsWith("language-");
    if (isBlock) {
      return (
        <code className="block w-full rounded-md bg-surface-0 px-3 py-2 font-mono text-[12px] text-text-secondary whitespace-pre overflow-x-auto">
          {children}
        </code>
      );
    }
    return (
      <code className="rounded bg-surface-0 px-1 py-0.5 font-mono text-[12px] text-text-secondary">
        {children}
      </code>
    );
  },
  pre: ({ children }) => (
    <pre className="mb-2 last:mb-0 overflow-x-auto rounded-md bg-surface-0 p-3 font-mono text-[12px] text-text-secondary">
      {children}
    </pre>
  ),
  ul: ({ children }) => <ul className="mb-2 last:mb-0 list-disc pl-4 space-y-0.5">{children}</ul>,
  ol: ({ children }) => <ol className="mb-2 last:mb-0 list-decimal pl-4 space-y-0.5">{children}</ol>,
  li: ({ children }) => <li>{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="mb-2 last:mb-0 border-l-2 border-border-default pl-3 text-text-tertiary italic">
      {children}
    </blockquote>
  ),
  h1: ({ children }) => <h1 className="mb-1 text-[15px] font-semibold text-text-primary">{children}</h1>,
  h2: ({ children }) => <h2 className="mb-1 text-[14px] font-semibold text-text-primary">{children}</h2>,
  h3: ({ children }) => <h3 className="mb-1 text-[13px] font-semibold text-text-primary">{children}</h3>,
  a: ({ href, children }) => (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      className="underline underline-offset-2 text-text-secondary hover:text-text-primary transition-colors"
    >
      {children}
    </a>
  ),
  hr: () => <hr className="my-2 border-border-subtle" />,
  table: ({ children }) => (
    <div className="mb-2 last:mb-0 overflow-x-auto">
      <table className="w-full text-left text-[13px] border-collapse">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="border-b border-border-default">{children}</thead>,
  th: ({ children }) => <th className="py-1 pr-4 font-semibold text-text-primary">{children}</th>,
  td: ({ children }) => <td className="py-1 pr-4 text-text-secondary border-t border-border-subtle">{children}</td>,
};

const assistantComponents: Components = {
  ...userComponents,
  code: ({ children, className }) => {
    const isBlock = className?.startsWith("language-");
    if (isBlock) {
      return (
        <code className="block w-full rounded-md bg-surface-1 px-3 py-2 font-mono text-[12px] text-text-secondary whitespace-pre overflow-x-auto">
          {children}
        </code>
      );
    }
    return (
      <code className="rounded bg-surface-1 px-1 py-0.5 font-mono text-[12px] text-text-tertiary">
        {children}
      </code>
    );
  },
  pre: ({ children }) => (
    <pre className="mb-2 last:mb-0 overflow-x-auto rounded-md bg-surface-1 p-3 font-mono text-[12px] text-text-secondary">
      {children}
    </pre>
  ),
};

export function MarkdownMessage({ content, role }: MarkdownMessageProps) {
  const components = role === "user" ? userComponents : assistantComponents;
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
      {content}
    </ReactMarkdown>
  );
}
