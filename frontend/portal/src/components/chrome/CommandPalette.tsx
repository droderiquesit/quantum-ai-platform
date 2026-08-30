"use client";

import { useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { describeOutcome, platform } from "@/lib/api/client";
import { connections } from "@/lib/hooks/connections";
import { NAV_ITEMS } from "@/lib/nav";

interface Command {
  readonly id: string;
  readonly label: string;
  readonly hint: string;
  run(): void | Promise<void>;
}

/**
 * Keyboard access to every page and to the two actions that are useful from
 * anywhere.
 *
 * Nothing in here is decorative: each entry navigates, reconnects a feed, or
 * calls a route, and the result of a call is reported in the palette rather
 * than assumed.
 */
export function CommandPalette() {
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setCursor(0);
  }, []);

  const commands = useMemo<Command[]>(() => {
    const navigation: Command[] = NAV_ITEMS.map((item) => ({
      id: `go:${item.href}`,
      label: `Go to ${item.label}`,
      hint: item.reads.join("  "),
      run: () => router.push(item.href),
    }));
    return [
      ...navigation,
      {
        id: "action:reconnect",
        label: "Reconnect every feed",
        hint: "restarts each poll and stream registered on this page",
        run: () => {
          const count = connections.reconnectAll();
          setNote(`Reconnect requested for ${count} feed(s).`);
        },
      },
      {
        id: "action:cycle",
        label: "Run one cycle of the intelligence loop",
        hint: "POST /api/v1/cycle — requires the analyst role",
        run: async () => {
          setNote("Running one cycle…");
          const response = await platform.runCycle();
          setNote(
            response.outcome.kind === "ok"
              ? `Cycle ${response.outcome.data.cycle} ran; ${response.outcome.data.stages.length} stage(s) reported.`
              : describeOutcome(response.outcome),
          );
        },
      },
    ];
  }, [router]);

  const matches = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === "") return commands;
    return commands.filter(
      (command) =>
        command.label.toLowerCase().includes(needle) || command.hint.toLowerCase().includes(needle),
    );
  }, [commands, query]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((value) => !value);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      dialog.showModal();
      inputRef.current?.focus();
    }
    if (!open && dialog.open) dialog.close();
  }, [open]);

  const runAt = useCallback(
    (index: number) => {
      const command = matches[index];
      if (!command) return;
      close();
      void command.run();
    },
    [matches, close],
  );

  return (
    <>
      <button
        type="button"
        className="btn"
        data-variant="ghost"
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
        data-testid="command-palette-open"
        title="Open the command palette"
      >
        <span className="hidden sm:inline">Commands</span>
        <kbd className="num text-[10px] text-[color:var(--color-ink-faint)]">⌘K</kbd>
      </button>

      <dialog
        ref={dialogRef}
        className="w-[min(620px,94vw)] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] p-0 text-[color:var(--color-ink)] backdrop:bg-black/70"
        aria-label="Command palette"
        onClose={close}
        onCancel={close}
      >
        <input
          ref={inputRef}
          className="input h-[38px] border-0 border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] text-[13px]"
          placeholder="Jump to a page, or run a command"
          value={query}
          aria-label="Search commands"
          onChange={(event) => {
            setQuery(event.target.value);
            setCursor(0);
          }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setCursor((value) => Math.min(matches.length - 1, value + 1));
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setCursor((value) => Math.max(0, value - 1));
            } else if (event.key === "Enter") {
              event.preventDefault();
              runAt(cursor);
            }
          }}
        />
        <ul className="max-h-[46vh] overflow-auto" role="listbox" aria-label="Commands">
          {matches.length === 0 ? (
            <li className="px-3 py-4 text-[12px] text-[color:var(--color-ink-dim)]">
              Nothing matches “{query}”.
            </li>
          ) : (
            matches.map((command, index) => (
              <li key={command.id}>
                <button
                  type="button"
                  role="option"
                  aria-selected={index === cursor}
                  className={`flex w-full items-baseline gap-3 px-3 py-1.5 text-left ${
                    index === cursor ? "bg-[color:var(--color-accent-dim)]" : ""
                  }`}
                  onMouseEnter={() => setCursor(index)}
                  onClick={() => runAt(index)}
                >
                  <span className="text-[12.5px]">{command.label}</span>
                  <span className="num ml-auto truncate text-[10px] text-[color:var(--color-ink-faint)]">
                    {command.hint}
                  </span>
                </button>
              </li>
            ))
          )}
        </ul>
        {note ? (
          <p
            className="border-t border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-3 py-2 text-[11.5px] text-[color:var(--color-ink-dim)]"
            role="status"
          >
            {note}
          </p>
        ) : null}
      </dialog>

      {note && !open ? (
        <div
          className="fixed bottom-4 left-1/2 z-50 flex -translate-x-1/2 items-center gap-3 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2 shadow-lg"
          role="status"
          data-testid="command-result"
        >
          <span className="text-[12px]">{note}</span>
          <button
            type="button"
            className="btn"
            data-variant="ghost"
            onClick={() => setNote(null)}
            aria-label="Dismiss the command result"
          >
            ×
          </button>
        </div>
      ) : null}
    </>
  );
}
