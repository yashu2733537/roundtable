import type { ReactNode } from "react";
import { LogoMark } from "./Icon";

interface EmptyStateProps {
  title: string;
  description: string;
  /** Show the brand mark above the title. */
  mark?: boolean;
  /** Extra content below the description (hints, actions). */
  children?: ReactNode;
}

export default function EmptyState({
  title,
  description,
  mark,
  children,
}: EmptyStateProps) {
  return (
    <div className="empty-state">
      {mark ? <LogoMark className="empty-state__mark" /> : null}
      <p className="empty-state__title">{title}</p>
      <p className="empty-state__desc">{description}</p>
      {children}
    </div>
  );
}
