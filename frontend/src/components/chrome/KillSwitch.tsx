"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { describeOutcome } from "@/lib/api/client";
import { StatusChip } from "@/components/data/Bits";
import { usePlatform } from "./PlatformProvider";

/**
 * The halt control, in the chrome, on every page.
 *
 * It is deliberately two clicks and a typed reason. `POST /api/v1/kill-switch`
 * takes the reason as a query parameter and the platform records it against the
 * operator identity in the credential, so an empty one is a halt nobody can
 * explain afterwards. Clearing is the same route with DELETE and can be refused
 * by the platform with a 409, which is surfaced verbatim rather than retried.
 */
export function KillSwitch() {
  const { halted, busy, trip, clear, lastAction, dismissAction } = usePlatform();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [reason, setReason] = useState("");
  const [open, setOpen] = useState(false);

  const close = useCallback(() => {
    setOpen(false);
    dialogRef.current?.close();
  }, []);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  const onSubmit = useCallback(
    async (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmed = reason.trim();
      if (trimmed.length === 0) return;
      await trip(trimmed);
      setReason("");
      close();
    },
    [reason, trip, close],
  );

  const onClear = useCallback(async () => {
    await clear();
    close();
  }, [clear, close]);

  const tone = halted === null ? "neutral" : halted ? "bad" : "ok";
  const label = halted === null ? "halt unknown" : halted ? "halted" : "running";

  return (
    <>
      <div className="flex items-center gap-2">
        <StatusChip
          tone={tone}
          label={label}
          pulse={halted === true}
          title={
            halted === null
              ? "The platform has not answered /health yet, so the halt state is unknown."
              : halted
                ? "The platform is halted. No cycle will run."
                : "The platform is running."
          }
        />
        <button
          type="button"
          className="btn"
          data-variant={halted ? undefined : "danger"}
          onClick={() => setOpen(true)}
          disabled={busy}
          aria-haspopup="dialog"
          data-testid="kill-switch-open"
        >
          {halted ? "Clear halt" : "Kill switch"}
        </button>
      </div>

      <dialog
        ref={dialogRef}
        className="w-[min(560px,92vw)] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] p-0 text-[color:var(--color-ink)] backdrop:bg-black/70"
        aria-labelledby="kill-switch-title"
        onClose={() => setOpen(false)}
        onCancel={() => setOpen(false)}
      >
        <div className="border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-4 py-2.5">
          <h2 id="kill-switch-title" className="panel-title">
            {halted ? "Clear the platform halt" : "Halt the platform"}
          </h2>
        </div>

        <div className="flex flex-col gap-3 px-4 py-4">
          {halted ? (
            <>
              <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
                Clearing sends <code className="num">DELETE /api/v1/kill-switch</code>. The platform
                requires an operator identity and may refuse with a 409 if the condition that caused
                the halt still holds; the refusal is shown here rather than retried.
              </p>
              <div className="flex justify-end gap-2">
                <button type="button" className="btn" data-variant="ghost" onClick={close}>
                  Cancel
                </button>
                <button
                  type="button"
                  className="btn"
                  data-variant="primary"
                  onClick={onClear}
                  disabled={busy}
                  data-testid="kill-switch-clear"
                >
                  {busy ? "Clearing…" : "Clear halt"}
                </button>
              </div>
            </>
          ) : (
            <form onSubmit={onSubmit} className="flex flex-col gap-3">
              <p className="text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
                Halting stops the intelligence loop across every scope. It sends{" "}
                <code className="num">POST /api/v1/kill-switch</code> and records the reason against
                the credential this console holds.
              </p>
              <div>
                <label className="field-label" htmlFor="kill-switch-reason">
                  Reason (recorded in the event log)
                </label>
                <input
                  id="kill-switch-reason"
                  className="input"
                  value={reason}
                  onChange={(event) => setReason(event.target.value)}
                  placeholder="e.g. venue reconciliation break on cell eu-west"
                  autoComplete="off"
                  required
                  data-testid="kill-switch-reason"
                />
              </div>
              <div className="flex justify-end gap-2">
                <button type="button" className="btn" data-variant="ghost" onClick={close}>
                  Cancel
                </button>
                <button
                  type="submit"
                  className="btn"
                  data-variant="danger"
                  disabled={busy || reason.trim().length === 0}
                  data-testid="kill-switch-confirm"
                >
                  {busy ? "Halting…" : "Halt the platform"}
                </button>
              </div>
            </form>
          )}
        </div>
      </dialog>

      {lastAction ? (
        <div
          className="fixed bottom-4 right-4 z-50 flex max-w-[420px] items-start gap-3 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2.5 shadow-lg"
          role="alert"
          data-testid="kill-switch-result"
        >
          <span className="eyebrow pt-0.5">
            {lastAction.kind === "trip" ? "halt" : "clear"}
          </span>
          <p className="flex-1 text-[12px] leading-relaxed">
            {lastAction.outcome.kind === "ok"
              ? lastAction.kind === "trip"
                ? "The platform accepted the halt."
                : "The platform cleared the halt."
              : describeOutcome(lastAction.outcome)}
          </p>
          <button
            type="button"
            className="btn"
            data-variant="ghost"
            onClick={dismissAction}
            aria-label="Dismiss the kill switch result"
          >
            ×
          </button>
        </div>
      ) : null}
    </>
  );
}
