/**
 * **The studio drawer** (Task 13.0 RULE 1, and the cleanup the brief asks for).
 *
 * Everything the artist-first shell does *not* put on screen lives here, behind
 * one button: the parametric surfaces (Parameters, Constraints, Operations,
 * Generators, Motion), the component inspector, export, and — behind an explicit
 * switch — the engine's own diagnostics.
 *
 * This is where "remove all debug UI, raw JSON displays and CAD-like
 * terminology" is actually implemented, and it is implemented by **moving, not
 * deleting**:
 *
 * * The panels are named for what a designer wants ("Parameters", "Generators",
 *   "Motion") instead of what a systems engineer built ("Variables",
 *   "Procedural", "Dependency graph"). Same content, same engine calls.
 * * The dependency inspector and the engine activity log are still *there* —
 *   they are genuinely useful when something is wrong — but under a
 *   **Developer** section that is off by default, so the default surface has no
 *   JSON, no graph dump and no event log.
 * * The drawer is a *drawer*: it slides over the canvas, it does not shrink it,
 *   and closing it restores exactly the layout that was there before. Focus mode
 *   (Tab) closes it too, because "leave only the artwork" has to mean it.
 *
 * The tab list is composed by the caller (`App`) rather than declared here, for
 * the same reason `MagicBar` takes its macros as props: this component owns the
 * *chrome*, and the app owns which surfaces exist. A tab whose content has not
 * been built yet simply is not in the list — there is no "coming soon" panel.
 */

import { useEffect } from 'react';
import { Beaker, X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

export interface DrawerTab {
  id: string;
  label: string;
  icon: LucideIcon;
  /** Rendered when the tab is active. */
  content: ReactNode;
  /** **Developer-only** tabs are hidden unless the switch is on. */
  dev?: boolean;
}

export interface StudioDrawerProps {
  open: boolean;
  onClose: () => void;
  tabs: DrawerTab[];
  active: string;
  onActive: (id: string) => void;
  /** The developer switch — off by default, and off in focus mode. */
  dev: boolean;
  onDev: (dev: boolean) => void;
  disabled?: boolean;
}

export default function StudioDrawer({
  open,
  onClose,
  tabs,
  active,
  onActive,
  dev,
  onDev,
  disabled,
}: StudioDrawerProps) {
  // Escape closes the drawer, the same key that closes every other overlay.
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;
  const visible = tabs.filter((tab) => dev || !tab.dev);
  const current = visible.find((tab) => tab.id === active) ?? visible[0];
  if (!current) return null;

  return (
    <aside className="studio-drawer" data-testid="studio-drawer" aria-label="Studio">
      <header className="drawer-head">
        <nav className="drawer-tabs" role="tablist" aria-label="Studio sections">
          {visible.map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.id}
                type="button"
                role="tab"
                aria-selected={tab.id === current.id}
                className={`drawer-tab${tab.id === current.id ? ' active' : ''}`}
                data-testid={`drawer-tab-${tab.id}`}
                disabled={disabled}
                title={tab.label}
                onClick={() => onActive(tab.id)}
              >
                <Icon size={15} strokeWidth={1.75} aria-hidden="true" />
                <span>{tab.label}</span>
              </button>
            );
          })}
        </nav>
        <span className="drawer-head-spacer" />
        <button
          type="button"
          className={`drawer-dev${dev ? ' active' : ''}`}
          data-testid="drawer-dev"
          aria-pressed={dev}
          title={
            dev
              ? 'Hide the engine diagnostics'
              : 'Show the engine diagnostics (dependency graph, command JSON, activity log)'
          }
          onClick={() => onDev(!dev)}
        >
          <Beaker size={15} strokeWidth={1.75} aria-hidden="true" />
          <span>Developer</span>
        </button>
        <button
          type="button"
          className="icon"
          data-testid="drawer-close"
          title="Close the studio (Esc)"
          aria-label="Close the studio"
          onClick={onClose}
        >
          <X size={16} strokeWidth={2} aria-hidden="true" />
        </button>
      </header>

      <div className="drawer-body" role="tabpanel" aria-label={current.label}>
        {current.content}
      </div>
    </aside>
  );
}
