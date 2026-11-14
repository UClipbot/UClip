import { useState } from 'react';
import { ActionMenu } from './ActionMenu';

// Keep filter popups inside the WebView: native select popups can blur the
// Linux window and trigger its auto-hide policy.
export function FilterMenu({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string; disabled?: boolean }[];
  onChange: (value: string) => void;
}) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  return (
    <>
      <button
        type="button"
        className="filter-trigger"
        value={value}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={!!anchor}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
        onKeyDown={(event) => {
          if (!event.nativeEvent.isComposing && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
            event.preventDefault();
            event.stopPropagation();
            setAnchor(event.currentTarget);
          }
        }}
      >
        {options.find((option) => option.value === value)?.label}
      </button>
      {anchor && (
        <ActionMenu anchor={anchor} onClose={() => setAnchor(null)}>
          {options.map((option) => (
            <button
              type="button"
              key={option.value}
              value={option.value}
              role="menuitemradio"
              aria-checked={option.value === value}
              disabled={option.disabled}
              onClick={() => {
                onChange(option.value);
                setAnchor(null);
              }}
            >
              {option.label}
            </button>
          ))}
        </ActionMenu>
      )}
    </>
  );
}
