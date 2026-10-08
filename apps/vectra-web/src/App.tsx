/**
 * VECTRA — Premium UI (Task 14.2)
 *
 * Professional design tool interface inspired by Figma, Excalidraw, Procreate.
 * Rich dark theme with violet accents, proper typography, micro-interactions.
 */

import React, { useState } from 'react';
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  FlipHorizontal,
  FolderPlus,
  Hand,
  Keyboard,
  Lock,
  MoreHorizontal,
  MousePointer2,
  Paintbrush,
  PenTool,
  Pencil,
  Pipette,
  Plus,
  Redo2,
  Search,
  Settings,
  SlidersHorizontal,
  Sparkles,
  Square,
  SquareDashed,
  Type,
  Undo2,
  Unlock,
} from 'lucide-react';

type PanelTab = 'Layers' | 'Properties';
type ToolId = 'select' | 'paint' | 'draw' | 'vector' | 'shape' | 'text' | 'fill' | 'eyedropper' | 'pan' | 'settings';
type LayerKind = 'group' | 'paint' | 'vector' | 'text';

interface DockButtonProps {
  icon: React.ReactNode;
  label: string;
  id: ToolId;
  active?: boolean;
  onClick?: () => void;
  indicator?: boolean;
}

interface LayerRowProps {
  name: string;
  icon: string;
  depth: number;
  kind: LayerKind;
  selected?: boolean;
  expanded?: boolean;
  hidden?: boolean;
  locked?: boolean;
}

interface PropInputProps {
  label: string;
  value: string;
  className?: string;
}

const HORIZONTAL_RULER_TICKS = [
  { id: 'hr-0', label: '' },
  { id: 'hr-50', label: '50' },
  { id: 'hr-100', label: '100' },
  { id: 'hr-150', label: '150' },
  { id: 'hr-200', label: '200' },
  { id: 'hr-250', label: '250' },
  { id: 'hr-300', label: '300' },
  { id: 'hr-350', label: '350' },
  { id: 'hr-400', label: '400' },
  { id: 'hr-450', label: '450' },
  { id: 'hr-500', label: '500' },
  { id: 'hr-550', label: '550' },
  { id: 'hr-600', label: '600' },
  { id: 'hr-650', label: '650' },
  { id: 'hr-700', label: '700' },
  { id: 'hr-750', label: '750' },
  { id: 'hr-800', label: '800' },
  { id: 'hr-850', label: '850' },
  { id: 'hr-900', label: '900' },
  { id: 'hr-950', label: '950' },
  { id: 'hr-1000', label: '1000' },
  { id: 'hr-1050', label: '1050' },
  { id: 'hr-1100', label: '1100' },
  { id: 'hr-1150', label: '1150' },
];

const VERTICAL_RULER_TICKS = [
  { id: 'vr-0', label: '' },
  { id: 'vr-50', label: '50' },
  { id: 'vr-100', label: '100' },
  { id: 'vr-150', label: '150' },
  { id: 'vr-200', label: '200' },
  { id: 'vr-250', label: '250' },
  { id: 'vr-300', label: '300' },
  { id: 'vr-350', label: '350' },
  { id: 'vr-400', label: '400' },
  { id: 'vr-450', label: '450' },
  { id: 'vr-500', label: '500' },
  { id: 'vr-550', label: '550' },
  { id: 'vr-600', label: '600' },
  { id: 'vr-650', label: '650' },
  { id: 'vr-700', label: '700' },
  { id: 'vr-750', label: '750' },
  { id: 'vr-800', label: '800' },
  { id: 'vr-850', label: '850' },
];

export default function App() {
  return <VectraEditor />;
}

export const VectraEditor = () => {
  const [activeTab, setActiveTab] = useState<PanelTab>('Layers');
  const [activeTool, setActiveTool] = useState<ToolId>('paint');

  return (
    <div
      className="flex h-screen w-screen overflow-hidden bg-[#0d0d0f] text-[#f0f0f2] antialiased selection:bg-[#8b5cf6]/30"
      style={{ fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, sans-serif' }}
    >
      <section className="flex h-full w-full flex-col overflow-hidden">
        {/* Top Bar */}
        <header className="flex h-12 shrink-0 items-center justify-between border-b border-[rgba(255,255,255,0.07)] bg-[#141416]/95 px-4 shadow-[0_1px_0_rgba(255,255,255,0.03)] backdrop-blur-xl">
          <div className="flex min-w-0 items-center gap-5">
            <div className="flex items-center gap-2.5" aria-label="Vectra editor">
              <div className="relative flex h-7 w-7 items-center justify-center rounded-lg bg-[#1a1a1e] text-[#8b5cf6] shadow-[0_0_26px_rgba(139,92,246,0.35)]">
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M12 2.5V21.5M3.8 7.25L20.2 16.75M20.2 7.25L3.8 16.75" stroke="currentColor" strokeWidth="2.4" strokeLinecap="square" />
                  <path d="M6.8 3.9L17.2 20.1M17.2 3.9L6.8 20.1" stroke="currentColor" strokeWidth="1.4" strokeLinecap="square" opacity="0.8" />
                </svg>
              </div>
              <strong className="text-sm font-semibold tracking-[-0.01em] text-[#f0f0f2]">Vectra</strong>
            </div>

            <nav className="hidden items-center gap-1 text-xs text-[#6b6b7a] md:flex" aria-label="Document breadcrumb">
              <button className="rounded-md px-2 py-1 transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button">
                <span>Botanical system</span>
              </button>
              <ChevronRight size={13} aria-hidden="true" />
              <button className="rounded-md px-2 py-1 text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                <span>Untitled Design</span>
              </button>
            </nav>
          </div>

          <div className="flex items-center gap-2 rounded-full border border-[rgba(255,255,255,0.07)] bg-[#1a1a1e] px-1.5 py-1 shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
            <button className="flex h-7 w-7 items-center justify-center rounded-full text-[#6b6b7a] transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button" aria-label="Undo">
              <Undo2 size={15} />
            </button>
            <button className="flex h-7 w-7 items-center justify-center rounded-full text-[#6b6b7a] transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button" aria-label="Redo">
              <Redo2 size={15} />
            </button>
            <span className="mx-1 h-4 w-px bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />
            <span className="flex items-center gap-1.5 px-2 text-xs text-[#6b6b7a]">
              <span className="h-1.5 w-1.5 rounded-full bg-[#22c55e] shadow-[0_0_12px_rgba(34,197,94,0.7)]" aria-hidden="true" />
              <span>Saved</span>
            </span>
          </div>

          <div className="flex items-center gap-3">
            <button className="hidden h-8 items-center gap-1.5 rounded-full border border-[rgba(255,255,255,0.07)] bg-[#1a1a1e] px-3 text-xs font-medium text-[#f0f0f2] transition-colors hover:bg-[#1f1f24] sm:flex" type="button" aria-label="Zoom control">
              <span>112%</span>
              <ChevronDown size={13} className="text-[#6b6b7a]" aria-hidden="true" />
            </button>
          </div>
        </header>

        <div className="flex min-h-0 flex-1 overflow-hidden">
          {/* Left Dock */}
          <aside className="flex w-[52px] shrink-0 flex-col items-center border-r border-[rgba(255,255,255,0.07)] bg-[#141416] py-2 shadow-[1px_0_0_rgba(255,255,255,0.025)]" aria-label="Tools">
            <div className="flex w-full flex-col items-center gap-1.5 px-1.5">
              <DockButton icon={<MousePointer2 size={19} />} label="Select" id="select" active={activeTool === 'select'} onClick={() => setActiveTool('select')} />
              <DockButton icon={<Paintbrush size={19} />} label="Paint" id="paint" active={activeTool === 'paint'} onClick={() => setActiveTool('paint')} />
              <DockButton icon={<Pencil size={19} />} label="Draw" id="draw" active={activeTool === 'draw'} onClick={() => setActiveTool('draw')} />
              <DockButton icon={<PenTool size={19} />} label="Pen vector" id="vector" active={activeTool === 'vector'} onClick={() => setActiveTool('vector')} indicator />
              <DockButton icon={<Square size={19} />} label="Shape" id="shape" active={activeTool === 'shape'} onClick={() => setActiveTool('shape')} />
              <DockButton icon={<Type size={19} />} label="Text" id="text" active={activeTool === 'text'} onClick={() => setActiveTool('text')} />
              <DockButton icon={<Sparkles size={18} />} label="Smart Fill" id="fill" active={activeTool === 'fill'} onClick={() => setActiveTool('fill')} />
              <DockButton icon={<Pipette size={18} />} label="Eyedropper" id="eyedropper" active={activeTool === 'eyedropper'} onClick={() => setActiveTool('eyedropper')} />
              <DockButton icon={<Hand size={18} />} label="Pan" id="pan" active={activeTool === 'pan'} onClick={() => setActiveTool('pan')} />
            </div>

            <div className="my-3 h-px w-7 bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />

            <button className="relative h-11 w-11 rounded-xl transition-colors hover:bg-[#1f1f24]" type="button" aria-label="Color swatches">
              <span className="absolute left-2 top-2 h-6 w-6 rounded-full border-2 border-[#f0f0f2] bg-[#8b5cf6] shadow-[0_0_18px_rgba(139,92,246,0.4)]" />
              <span className="absolute bottom-2 right-2 h-6 w-6 rounded-full border-2 border-[#141416] bg-[#2dd4bf]" />
              <span className="absolute right-1.5 top-1.5 h-3.5 w-3.5 rounded-full border border-[#141416] bg-[#f87171]" />
            </button>

            <div className="mt-auto flex w-full flex-col items-center gap-1.5 px-1.5 pb-1">
              <DockButton icon={<SlidersHorizontal size={18} />} label="Adjustments" id="settings" active={activeTool === 'settings'} onClick={() => setActiveTool('settings')} />
              <button className="flex h-10 w-10 items-center justify-center rounded-xl text-[#6b6b7a] transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button" aria-label="Keyboard shortcuts">
                <Keyboard size={18} />
              </button>
              <button className="flex h-10 w-10 items-center justify-center rounded-xl text-[#6b6b7a] transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button" aria-label="Settings">
                <Settings size={18} />
              </button>
            </div>
          </aside>

          {/* Canvas Area */}
          <main className="relative min-w-0 flex-1 overflow-hidden bg-[#0d0d0f]" aria-label="Canvas workspace">
            {/* Dot grid */}
            <div className="absolute inset-0 opacity-[0.38]" style={{ backgroundImage: 'radial-gradient(circle at 1px 1px, rgba(255,255,255,0.12) 1px, transparent 0)', backgroundSize: '18px 18px' }} aria-hidden="true" />
            {/* Subtle gradients */}
            <div className="absolute inset-0 bg-[radial-gradient(circle_at_48%_42%,rgba(139,92,246,0.09),transparent_33%),radial-gradient(circle_at_62%_68%,rgba(45,212,191,0.07),transparent_28%)]" aria-hidden="true" />

            {/* Horizontal Ruler */}
            <div className="absolute left-7 right-0 top-0 z-20 flex h-7 items-end overflow-hidden border-b border-[rgba(255,255,255,0.07)] bg-[#141416]/90 backdrop-blur-xl">
              {HORIZONTAL_RULER_TICKS.map(tick => (
                <div key={tick.id} className="relative h-full w-[50px] shrink-0">
                  <span className="absolute bottom-0 left-0 h-1.5 w-px bg-[rgba(255,255,255,0.16)]" aria-hidden="true" />
                  <span className="absolute bottom-0 left-[10px] h-1 w-px bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute bottom-0 left-[20px] h-1 w-px bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute bottom-0 left-[30px] h-1 w-px bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute bottom-0 left-[40px] h-1 w-px bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  {tick.label !== '' && <span className="absolute bottom-1.5 left-1 text-[9px] leading-none text-[#6b6b7a]">{tick.label}</span>}
                </div>
              ))}
            </div>

            {/* Vertical Ruler */}
            <div className="absolute bottom-0 left-0 top-0 z-20 flex w-7 flex-col items-end overflow-hidden border-r border-[rgba(255,255,255,0.07)] bg-[#141416]/90 backdrop-blur-xl">
              <div className="h-7 w-full border-b border-[rgba(255,255,255,0.07)] bg-[#141416]" aria-hidden="true" />
              {VERTICAL_RULER_TICKS.map(tick => (
                <div key={tick.id} className="relative h-[50px] w-full shrink-0">
                  <span className="absolute right-0 top-0 h-px w-1.5 bg-[rgba(255,255,255,0.16)]" aria-hidden="true" />
                  <span className="absolute right-0 top-[10px] h-px w-1 bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute right-0 top-[20px] h-px w-1 bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute right-0 top-[30px] h-px w-1 bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  <span className="absolute right-0 top-[40px] h-px w-1 bg-[rgba(255,255,255,0.09)]" aria-hidden="true" />
                  {tick.label !== '' && <span className="absolute right-1 top-1 origin-right -rotate-90 whitespace-nowrap text-[9px] leading-none text-[#6b6b7a]">{tick.label}</span>}
                </div>
              ))}
            </div>

            {/* Artboard */}
            <div className="absolute left-1/2 top-1/2 h-[640px] w-[780px] -translate-x-1/2 -translate-y-[47%] rounded-[28px] border border-[rgba(255,255,255,0.07)] bg-[#101012]/55 shadow-[0_34px_120px_rgba(0,0,0,0.38),inset_0_1px_0_rgba(255,255,255,0.04)]" aria-hidden="true" />

            {/* Selected object with bounding box */}
            <div className="absolute left-1/2 top-1/2 h-[238px] w-[304px] -translate-x-[10%] -translate-y-[35%]" aria-label="Selected violet petal">
              {/* Bounding box */}
              <div className="absolute inset-0 border border-[#3b82f6] shadow-[0_0_0_1px_rgba(59,130,246,0.22),0_0_28px_rgba(59,130,246,0.14)]">
                <span className="absolute -left-1.5 -top-1.5 h-3 w-3 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -top-1.5 left-1/2 h-3 w-3 -translate-x-1/2 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -right-1.5 -top-1.5 h-3 w-3 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -left-1.5 top-1/2 h-3 w-3 -translate-y-1/2 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -right-1.5 top-1/2 h-3 w-3 -translate-y-1/2 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -bottom-1.5 -left-1.5 h-3 w-3 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -bottom-1.5 left-1/2 h-3 w-3 -translate-x-1/2 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
                <span className="absolute -bottom-1.5 -right-1.5 h-3 w-3 rounded-[3px] border border-[#3b82f6] bg-[#f0f0f2]" aria-hidden="true" />
              </div>

              {/* Floating HUD */}
              <div className="absolute -top-[58px] left-1/2 z-30 flex -translate-x-1/2 items-center gap-1 rounded-2xl border border-[rgba(255,255,255,0.09)] bg-[#141416]/95 p-1.5 shadow-[0_18px_45px_rgba(0,0,0,0.46),inset_0_1px_0_rgba(255,255,255,0.04)] backdrop-blur-xl">
                <button className="flex h-8 items-center gap-1.5 rounded-xl px-2.5 text-xs font-medium text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                  <Copy size={14} aria-hidden="true" />
                  <span>Duplicate</span>
                </button>
                <span className="h-4 w-px bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />
                <button className="flex h-8 items-center gap-1.5 rounded-xl px-2.5 text-xs font-medium text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                  <FlipHorizontal size={14} aria-hidden="true" />
                  <span>Flip</span>
                </button>
                <span className="h-4 w-px bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />
                <button className="flex h-8 cursor-not-allowed items-center gap-1.5 rounded-xl px-2.5 text-xs font-medium text-[#6b6b7a] opacity-55" type="button" disabled>
                  <SquareDashed size={14} aria-hidden="true" />
                  <span>Boolean</span>
                </button>
                <span className="h-4 w-px bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />
                <button className="flex h-8 w-8 items-center justify-center rounded-xl text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button" aria-label="More canvas actions">
                  <MoreHorizontal size={14} />
                </button>
              </div>
            </div>

            <div className="absolute bottom-4 left-5 z-30 hidden rounded-full border border-[rgba(255,255,255,0.07)] bg-[#141416]/80 px-3 py-1.5 text-[11px] text-[#6b6b7a] backdrop-blur-xl md:block">
              <span>Tab — Focus Mode</span>
            </div>
          </main>

          {/* Right Panel */}
          <aside className="flex w-72 shrink-0 flex-col border-l border-[rgba(255,255,255,0.07)] bg-[#141416] shadow-[-14px_0_42px_rgba(0,0,0,0.18)]" aria-label="Inspector">
            <div className="flex h-11 shrink-0 border-b border-[rgba(255,255,255,0.07)] p-1.5">
              <button className={`flex-1 rounded-xl text-xs font-semibold transition-colors ${activeTab === 'Layers' ? 'bg-[#1a1a1e] text-[#f0f0f2] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]' : 'text-[#6b6b7a] hover:bg-[#1f1f24] hover:text-[#f0f0f2]'}`} type="button" onClick={() => setActiveTab('Layers')}>
                <span>Layers</span>
              </button>
              <button className={`flex-1 rounded-xl text-xs font-semibold transition-colors ${activeTab === 'Properties' ? 'bg-[#1a1a1e] text-[#f0f0f2] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]' : 'text-[#6b6b7a] hover:bg-[#1f1f24] hover:text-[#f0f0f2]'}`} type="button" onClick={() => setActiveTab('Properties')}>
                <span>Properties</span>
              </button>
            </div>

            {activeTab === 'Layers' ? (
              <section className="flex min-h-0 flex-1 flex-col" aria-label="Layers panel">
                <div className="border-b border-[rgba(255,255,255,0.07)] p-3">
                  <label className="relative block" aria-label="Search layers">
                    <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-[#6b6b7a]" aria-hidden="true" />
                    <input className="h-9 w-full rounded-xl border border-[rgba(255,255,255,0.07)] bg-[#1a1a1e] pl-9 pr-3 text-xs text-[#f0f0f2] outline-none transition-colors placeholder:text-[#6b6b7a] focus:border-[#8b5cf6]/70 focus:shadow-[0_0_0_3px_rgba(139,92,246,0.12)]" type="text" placeholder="Search layers" />
                  </label>
                </div>

                <div className="flex-1 overflow-y-auto py-1.5">
                  <LayerRow name="Composition" icon="▣" depth={0} kind="group" expanded />
                  <LayerRow name="Background wash" icon="◉" depth={1} kind="paint" />
                  <LayerRow name="Petal 1" icon="◇" depth={1} kind="vector" selected />
                  <LayerRow name="Petal 2" icon="◇" depth={1} kind="vector" />
                  <LayerRow name="Stem" icon="◇" depth={1} kind="vector" />
                  <LayerRow name="Vectra Sans note" icon="T" depth={1} kind="text" hidden />
                  <LayerRow name="Grid guide" icon="◇" depth={0} kind="vector" locked />
                </div>

                <div className="grid grid-cols-2 gap-2 border-t border-[rgba(255,255,255,0.07)] p-3">
                  <button className="flex h-9 items-center justify-center gap-1.5 rounded-xl bg-[#1a1a1e] text-xs font-medium text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                    <Plus size={14} aria-hidden="true" />
                    <span>Add Layer</span>
                  </button>
                  <button className="flex h-9 items-center justify-center gap-1.5 rounded-xl bg-[#1a1a1e] text-xs font-medium text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                    <FolderPlus size={14} aria-hidden="true" />
                    <span>Group</span>
                  </button>
                </div>
              </section>
            ) : (
              <section className="flex min-h-0 flex-1 flex-col overflow-y-auto" aria-label="Properties panel">
                <div className="border-b border-[rgba(255,255,255,0.07)] p-3">
                  <div className="mb-3 flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-[0.14em] text-[#6b6b7a]">Transform</h2>
                    <ChevronDown size={14} className="text-[#6b6b7a]" aria-hidden="true" />
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    <PropInput label="X" value="258" />
                    <PropInput label="Y" value="186" />
                    <PropInput label="W" value="304" />
                    <PropInput label="H" value="238" />
                    <PropInput label="R" value="0°" className="col-span-2" />
                  </div>
                </div>

                <div className="border-b border-[rgba(255,255,255,0.07)] p-3">
                  <div className="mb-3 flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-[0.14em] text-[#6b6b7a]">Appearance</h2>
                    <ChevronDown size={14} className="text-[#6b6b7a]" aria-hidden="true" />
                  </div>
                  <div className="space-y-3">
                    <div className="flex items-center justify-between rounded-xl bg-[#1a1a1e] px-3 py-2">
                      <span className="flex items-center gap-2 text-xs text-[#f0f0f2]"><span className="h-4 w-4 rounded-md bg-[#8b5cf6] shadow-[0_0_16px_rgba(139,92,246,0.35)]" aria-hidden="true" /><span>Fill</span></span>
                      <span className="font-mono text-[11px] text-[#6b6b7a]">82%</span>
                    </div>
                    <div className="flex items-center justify-between rounded-xl bg-[#1a1a1e] px-3 py-2">
                      <span className="flex items-center gap-2 text-xs text-[#f0f0f2]"><span className="h-4 w-4 rounded-md border-2 border-[#3b82f6]" aria-hidden="true" /><span>Stroke</span></span>
                      <span className="font-mono text-[11px] text-[#6b6b7a]">1.8px</span>
                    </div>
                    <div className="rounded-xl bg-[#1a1a1e] px-3 py-2.5">
                      <div className="mb-2 flex items-center justify-between text-xs">
                        <span className="text-[#f0f0f2]">Opacity</span>
                        <span className="font-mono text-[11px] text-[#6b6b7a]">100%</span>
                      </div>
                      <div className="h-1.5 overflow-hidden rounded-full bg-[#0d0d0f]">
                        <div className="h-full w-full rounded-full bg-[#8b5cf6] shadow-[0_0_14px_rgba(139,92,246,0.55)]" />
                      </div>
                    </div>
                  </div>
                </div>

                <div className="p-3">
                  <div className="mb-3 flex items-center justify-between">
                    <h2 className="text-xs font-semibold uppercase tracking-[0.14em] text-[#6b6b7a]">Geometry</h2>
                    <ChevronDown size={14} className="text-[#6b6b7a]" aria-hidden="true" />
                  </div>
                  <div className="space-y-2 rounded-xl bg-[#1a1a1e] p-3">
                    <div className="flex justify-between text-xs">
                      <span className="text-[#6b6b7a]">Nodes</span>
                      <span className="text-[#f0f0f2]">8 anchors</span>
                    </div>
                    <div className="flex justify-between text-xs">
                      <span className="text-[#6b6b7a]">Path</span>
                      <span className="text-[#f0f0f2]">Closed</span>
                    </div>
                    <button className="mt-2 h-9 w-full rounded-xl bg-[#0d0d0f] text-xs font-semibold text-[#f0f0f2] transition-colors hover:bg-[#1f1f24]" type="button">
                      <span>Break Path</span>
                    </button>
                  </div>
                </div>
              </section>
            )}

            <button className="mt-auto flex h-10 shrink-0 items-center justify-center gap-1 border-t border-[rgba(255,255,255,0.07)] bg-[#1a1a1e] text-xs font-medium text-[#6b6b7a] transition-colors hover:bg-[#1f1f24] hover:text-[#f0f0f2]" type="button">
              <span>Studio</span>
              <ChevronRight size={13} aria-hidden="true" />
            </button>
          </aside>
        </div>

        {/* Bottom Status Bar */}
        <footer className="flex h-9 shrink-0 items-center justify-between border-t border-[rgba(255,255,255,0.07)] bg-[#141416] px-3 shadow-[0_-1px_0_rgba(255,255,255,0.03)]">
          <div className="flex min-w-0 items-center gap-3">
            <span className="rounded-full bg-[#8b5cf6]/16 px-2.5 py-1 text-[11px] font-bold tracking-[0.12em] text-[#c4b5fd] shadow-[0_0_18px_rgba(139,92,246,0.16)]">NODE</span>
            <span className="hidden text-xs text-[#6b6b7a] sm:inline">8 nodes</span>
            <span className="hidden h-1 w-1 rounded-full bg-[rgba(255,255,255,0.16)] sm:inline" aria-hidden="true" />
            <span className="hidden text-xs text-[#6b6b7a] sm:inline">Closed path</span>
            <span className="h-4 w-px bg-[rgba(255,255,255,0.07)]" aria-hidden="true" />
            <span className="flex items-center gap-1.5 text-xs text-[#f0f0f2]"><span className="h-3.5 w-3.5 rounded bg-[#8b5cf6]" aria-hidden="true" /><span>Fill</span></span>
            <span className="flex items-center gap-1.5 text-xs text-[#f0f0f2]"><span className="h-3.5 w-3.5 rounded border-2 border-[#3b82f6]" aria-hidden="true" /><span>Stroke</span></span>
            <span className="hidden text-xs text-[#6b6b7a] md:inline">Opacity 100%</span>
            <span className="hidden text-xs text-[#6b6b7a] md:inline">Normal</span>
          </div>
          <div className="flex items-center gap-3 font-mono text-[11px] text-[#6b6b7a]">
            <span className="hidden lg:inline">Tab Focus Mode</span>
            <span>X 258</span>
            <span>Y 186</span>
            <span>W 304</span>
            <span>H 238</span>
          </div>
        </footer>
      </section>
    </div>
  );
};

function DockButton({ icon, label, active, onClick, indicator }: DockButtonProps) {
  return (
    <div className="relative flex w-full justify-center">
      <button
        className={`relative flex h-10 w-10 items-center justify-center rounded-xl transition-all ${
          active ? 'bg-[#8b5cf6] text-white shadow-[0_0_24px_rgba(139,92,246,0.5),inset_0_1px_0_rgba(255,255,255,0.22)]' : 'text-[#6b6b7a] hover:bg-[#1f1f24] hover:text-[#f0f0f2]'
        }`}
        type="button"
        aria-label={label}
        aria-pressed={active ? 'true' : 'false'}
        onClick={onClick}
      >
        {icon}
      </button>
      {indicator && <span className="absolute bottom-1.5 right-2 h-1.5 w-1.5 rounded-full bg-[#2dd4bf] shadow-[0_0_10px_rgba(45,212,191,0.8)]" aria-hidden="true" />}
    </div>
  );
}

function LayerRow({ name, icon, depth, kind, selected, expanded, hidden, locked }: LayerRowProps) {
  const glyphColor = kind === 'paint' ? 'text-[#f87171]' : kind === 'vector' ? 'text-[#2dd4bf]' : kind === 'group' ? 'text-[#8b5cf6]' : 'text-[#fbbf24]';

  return (
    <div className={`group relative mx-2 flex h-9 items-center rounded-xl px-2 transition-colors ${selected ? 'bg-[#8b5cf6]/18 text-[#f0f0f2] shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]' : 'text-[#6b6b7a] hover:bg-[#1f1f24] hover:text-[#f0f0f2]'}`}>
      {selected && <span className="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full bg-[#8b5cf6] shadow-[0_0_12px_rgba(139,92,246,0.75)]" aria-hidden="true" />}
      <span style={{ width: depth * 15 }} className="shrink-0" aria-hidden="true" />
      <span className="flex h-4 w-4 shrink-0 items-center justify-center text-[#6b6b7a]" aria-hidden="true">
        {kind === 'group' && (expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />)}
      </span>
      <span className={`flex w-5 shrink-0 items-center justify-center font-mono text-[11px] ${glyphColor}`} aria-hidden="true">
        {icon}
      </span>
      <span className={`ml-1.5 flex-1 truncate text-xs ${selected ? 'font-semibold' : 'font-medium'} ${hidden ? 'opacity-50' : ''}`}>{name}</span>
      <span className="flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
        {locked ? <Lock size={13} className="text-[#6b6b7a]" aria-hidden="true" /> : hidden ? <EyeOff size={13} className="text-[#6b6b7a]" aria-hidden="true" /> : <Eye size={13} className="text-[#6b6b7a]" aria-hidden="true" />}
        {locked ? <Lock size={13} className="text-[#6b6b7a]" aria-hidden="true" /> : <Unlock size={13} className="text-[#6b6b7a]" aria-hidden="true" />}
      </span>
      <span className="flex w-[30px] items-center justify-end gap-1 group-hover:hidden">
        {locked && <Lock size={13} className="text-[#6b6b7a]" aria-hidden="true" />}
        {hidden && <EyeOff size={13} className="text-[#6b6b7a]" aria-hidden="true" />}
      </span>
    </div>
  );
}

function PropInput({ label, value, className = '' }: PropInputProps) {
  return (
    <label className={`flex h-9 items-center overflow-hidden rounded-xl border border-[rgba(255,255,255,0.07)] bg-[#1a1a1e] ${className}`}>
      <span className="flex h-full w-8 items-center justify-center border-r border-[rgba(255,255,255,0.07)] bg-[#141416] text-[10px] font-semibold text-[#6b6b7a]">{label}</span>
      <input className="h-full min-w-0 flex-1 bg-transparent px-2 font-mono text-xs text-[#f0f0f2] outline-none transition-colors focus:bg-[#1f1f24]" type="text" defaultValue={value} aria-label={label} />
    </label>
  );
}
