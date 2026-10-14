import { useEffect, useMemo, useRef, useState } from "react";
import Modal from "./Modal";
import { IconCommand } from "./Icon";

export interface Command {
  id: string;
  label: string;
  /** Group heading shown on the right of each row. */
  group: string;
  run: () => void;
}

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: Command[];
}

export default function CommandPalette({
  open,
  onClose,
  commands,
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter(
      (cmd) =>
        cmd.label.toLowerCase().includes(q) ||
        cmd.group.toLowerCase().includes(q),
    );
  }, [commands, query]);

  // Reset state whenever the palette opens; keep input focused.
  useEffect(() => {
    if (open) {
      setQuery("");
      setActiveIndex(0);
      // showModal focuses the first focusable element (the input) already;
      // this covers browsers that skip it.
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Clamp selection when the result list shrinks.
  useEffect(() => {
    setActiveIndex((i) => (i >= filtered.length ? 0 : i));
  }, [filtered.length]);

  const runCommand = (cmd: Command | undefined) => {
    if (!cmd) return;
    onClose();
    cmd.run();
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((i) => (filtered.length === 0 ? 0 : (i + 1) % filtered.length));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((i) =>
        filtered.length === 0 ? 0 : (i - 1 + filtered.length) % filtered.length,
      );
    } else if (event.key === "Enter") {
      event.preventDefault();
      runCommand(filtered[activeIndex]);
    }
  };

  return (
    <Modal open={open} onClose={onClose} label="Command palette" className="palette">
      <div className="palette__input-row">
        <IconCommand className="palette__icon" width={18} height={18} />
        <input
          ref={inputRef}
          type="text"
          className="palette__input"
          placeholder="Type a command…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={handleKeyDown}
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-autocomplete="list"
          aria-activedescendant={
            filtered.length > 0 ? `palette-option-${filtered[activeIndex].id}` : undefined
          }
          aria-label="Command search"
        />
      </div>
      <ul
        className="palette__list"
        id="palette-list"
        role="listbox"
        aria-label="Commands"
      >
        {filtered.length === 0 && (
          <li className="palette__empty" role="presentation">
            No commands match “{query.trim()}”.
          </li>
        )}
        {filtered.map((cmd, index) => (
          <li
            key={cmd.id}
            id={`palette-option-${cmd.id}`}
            role="option"
            aria-selected={index === activeIndex}
            className={
              "palette__option" +
              (index === activeIndex ? " palette__option--active" : "")
            }
            onMouseEnter={() => setActiveIndex(index)}
            onClick={() => runCommand(cmd)}
          >
            <span className="palette__label">{cmd.label}</span>
            <span className="palette__group">{cmd.group}</span>
          </li>
        ))}
      </ul>
    </Modal>
  );
}
