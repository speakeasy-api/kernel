interface OpenFolderButtonProps {
  onClick: () => void;
}

export function OpenFolderButton({ onClick }: OpenFolderButtonProps) {
  return (
    <button
      onClick={onClick}
      className="group flex w-full items-center justify-center gap-2.5 rounded-xl border border-dashed border-border-default py-3 text-[13px] font-medium text-text-tertiary transition-all duration-150 hover:border-border-strong hover:bg-surface-1/50 hover:text-text-secondary active:scale-[0.99] cursor-pointer"
    >
      <svg
        className="h-4 w-4 text-text-ghost transition-colors duration-150 group-hover:text-text-tertiary"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M2 4.5V12a1 1 0 001 1h10a1 1 0 001-1V6a1 1 0 00-1-1H8L6.5 3.5H3A1 1 0 002 4.5z" />
      </svg>
      Open folder
    </button>
  );
}
