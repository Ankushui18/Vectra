/**
 * The **Smart Component panel** (Task 10.6, RULE 1 + RULE 4).
 *
 * A property inspector, and nothing else: the engine's `component_view` arrives
 * here as data (`ComponentViewWire`) and everything this panel prints comes from
 * it — the headline, the prop labels, whether a prop is derived and what it
 * follows. The panel composes no sentences of its own and shows no graph, which
 * is the rule: the procedural machinery stays behind the engine boundary, and
 * what a designer sees is a name and some sliders.
 *
 * It is a *dumb* view on purpose (the house rule of this app, MES §15–16): it
 * holds no state, calls no engine, and returns every intent through a callback.
 * That is what makes it mountable in a test with no browser, no wasm and no
 * WebGPU — which is how the markup below is pinned, including the two states a
 * click cannot reach from here: a stale engine's sentence, and the empty state.
 */

import type { ComponentPropWire, ComponentViewWire } from '../engine/wire';

export interface ComponentPanelProps {
  /** The engine's view of the selection (`null` before the first inspect). */
  component: ComponentViewWire | null;
  /** How many nodes are selected — what "Create Component" would group. */
  selectionCount: number;
  /** The engine is loaded and the document is stable. */
  ready: boolean;
  /** True ⟺ the wasm predates Task 10.6 (see `engineIsOlderThanUi`). */
  engineIsOlder: boolean;
  /** The engine's own one-sentence explanation, printed verbatim. */
  staleSentence: string;
  /** The Icon Studio's size ladder, as typed. */
  iconSizes: string;
  onIconSizes: (value: string) => void;
  onCreate: () => void;
  onPlaceInstance: (master: string) => void;
  onGenerateIconSet: (master: string) => void;
  /** A slider or a swatch moved: the *typed* value is built by the caller. */
  onSetProp: (target: string, prop: ComponentPropWire, value: number | string) => void;
}

export function ComponentPanel({
  component,
  selectionCount,
  ready,
  engineIsOlder,
  staleSentence,
  iconSizes,
  onIconSizes,
  onCreate,
  onPlaceInstance,
  onGenerateIconSet,
  onSetProp,
}: ComponentPanelProps) {
  const id = component?.id ?? '';
  const isMaster = component?.role === 'master' && Boolean(id);
  const masterName = component?.master
    ? component.masters.find((master) => master.id === component.master)?.name ?? 'a master'
    : '';
  return (
    <section className="panel" data-testid="component-panel">
      <h2>
        Component{' '}
        <span className="sub" data-testid="component-headline">
          {component?.headline ?? 'select artwork'}
        </span>
      </h2>
      {/* A wasm built before Task 10.6 has none of these methods. One
          sentence and a command beats a TypeError on the first click. */}
      {engineIsOlder ? (
        <p className="magic-hint" data-testid="engine-outdated">
          {staleSentence}
        </p>
      ) : null}
      {component?.role === 'instance' && component.master ? (
        <p className="magic-hint" data-testid="component-master">
          referencing {masterName}
        </p>
      ) : null}
      {component && component.props.length ? (
        <ul className="prop-list" data-testid="component-props">
          {component.props.map((prop) => (
            <li key={prop.key} data-testid={`prop-${prop.key}`}>
              <span className="prop-label">
                {prop.label}
                {prop.derived && prop.from ? (
                  <em className="prop-derived" title={`follows ${prop.from}`}>
                    {' '}
                    ∝ {prop.from}
                  </em>
                ) : null}
              </span>
              {prop.ty === 'color' ? (
                <input
                  type="color"
                  value={prop.color ?? '#ffffff'}
                  data-testid={`prop-input-${prop.key}`}
                  onChange={(event) => onSetProp(id, prop, event.target.value)}
                />
              ) : (
                <>
                  <input
                    type="range"
                    min={prop.min}
                    max={prop.max}
                    step={(prop.max - prop.min) / 200 || 1}
                    value={prop.value ?? prop.min}
                    data-testid={`prop-input-${prop.key}`}
                    onChange={(event) => onSetProp(id, prop, Number(event.target.value))}
                  />
                  <span className="prop-value" data-testid={`prop-value-${prop.key}`}>
                    {(prop.value ?? 0).toFixed(1)}
                  </span>
                </>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <p className="empty">
          Select a component, or artwork to make one. Props appear here as sliders — the
          graph stays hidden.
        </p>
      )}
      <div className="btn-row">
        <button
          data-testid="component-create"
          disabled={!ready || selectionCount === 0}
          title="Make the selection a Smart Component with props"
          onClick={onCreate}
        >
          Create Component
        </button>
        <button
          data-testid="component-instantiate"
          disabled={!ready || !isMaster}
          title="Place another instance of this master"
          onClick={() => id && onPlaceInstance(id)}
        >
          Place Instance
        </button>
      </div>
      <div className="btn-row">
        <input
          className="icon-sizes"
          data-testid="icon-sizes"
          value={iconSizes}
          onChange={(event) => onIconSizes(event.target.value)}
          placeholder="16 32 48"
          aria-label="Icon sizes"
        />
        <button
          data-testid="icon-set"
          disabled={!ready || !id}
          title="One artboard per size; stroke and corners scale with it"
          onClick={() => id && onGenerateIconSet(id)}
        >
          Generate Icon Set
        </button>
      </div>
    </section>
  );
}

export default ComponentPanel;
