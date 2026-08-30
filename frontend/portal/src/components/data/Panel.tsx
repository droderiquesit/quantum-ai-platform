import type { ReactNode } from "react";

export function Panel({
  children,
  className = "",
  ...rest
}: {
  children: ReactNode;
  className?: string;
} & React.HTMLAttributes<HTMLElement>) {
  return (
    <section className={`panel ${className}`} {...rest}>
      {children}
    </section>
  );
}

export function PanelHead({
  title,
  id,
  meta,
  actions,
}: {
  title: string;
  /** Ties the heading to the region that owns it, for screen readers. */
  id?: string;
  meta?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="panel-head">
      <h2 className="panel-title" id={id}>
        {title}
      </h2>
      {meta ? <div className="flex min-w-0 items-center gap-2 text-[10px]">{meta}</div> : null}
      <div className="ml-auto flex items-center gap-1.5">{actions}</div>
    </header>
  );
}

export function PanelBody({
  children,
  flush = false,
  className = "",
}: {
  children: ReactNode;
  flush?: boolean;
  className?: string;
}) {
  return (
    <div className={`${flush ? "panel-body-flush" : "panel-body"} ${className}`}>{children}</div>
  );
}

/** A horizontally scrolling well for a table too wide for its column. */
export function TableWell({
  children,
  maxHeight = "none",
  label,
}: {
  children: ReactNode;
  maxHeight?: string;
  label: string;
}) {
  return (
    <div
      className="w-full overflow-auto"
      style={{ maxHeight }}
      tabIndex={0}
      role="region"
      aria-label={label}
    >
      {children}
    </div>
  );
}
