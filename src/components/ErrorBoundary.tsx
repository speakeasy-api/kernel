import React from "react";

interface State {
  error: Error | null;
}

export class ErrorBoundary extends React.Component<
  React.PropsWithChildren,
  State
> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex h-screen w-screen items-center justify-center bg-surface-1 p-8">
          <div className="max-w-md space-y-3 text-center">
            <h1 className="text-lg font-semibold text-text-primary">
              Something went wrong
            </h1>
            <pre className="overflow-auto rounded-md bg-surface-2 p-3 text-left text-xs text-text-secondary">
              {this.state.error.message}
            </pre>
            <button
              onClick={() => this.setState({ error: null })}
              className="rounded-md bg-surface-3 px-4 py-2 text-sm text-text-primary hover:bg-surface-4 transition-colors cursor-pointer"
            >
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
