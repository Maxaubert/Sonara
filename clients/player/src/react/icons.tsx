// Inline 24x24 icons drawn for this package; decorative only (the buttons
// carry the accessible names).
import type { ReactElement } from "react";

export type IconName = "play" | "pause" | "stop" | "previous" | "next" | "restart" | "volume" | "muted";

const shapes: Record<IconName, ReactElement> = {
  play: <polygon points="7 5 19 12 7 19" fill="currentColor" />,
  pause: (
    <>
      <rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor" />
      <rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor" />
    </>
  ),
  stop: <rect x="6" y="6" width="12" height="12" rx="1.5" fill="currentColor" />,
  previous: (
    <>
      <polygon points="18 6 10 12 18 18" fill="currentColor" />
      <rect x="6" y="6" width="2.5" height="12" rx="1" fill="currentColor" />
    </>
  ),
  next: (
    <>
      <polygon points="6 6 14 12 6 18" fill="currentColor" />
      <rect x="15.5" y="6" width="2.5" height="12" rx="1" fill="currentColor" />
    </>
  ),
  restart: (
    <>
      <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      <polyline points="4.5 3.5 4.5 8.5 9.5 8.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
    </>
  ),
  volume: (
    <>
      <polygon points="3 9 7 9 12 5 12 19 7 15 3 15" fill="currentColor" />
      <path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </>
  ),
  muted: (
    <>
      <polygon points="3 9 7 9 12 5 12 19 7 15 3 15" fill="currentColor" />
      <path d="M16 9.5l5 5M21 9.5l-5 5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </>
  ),
};

export function Icon({ name }: { name: IconName }): ReactElement {
  return (
    <svg className="sonara-player__icon" viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" focusable="false">
      {shapes[name]}
    </svg>
  );
}
