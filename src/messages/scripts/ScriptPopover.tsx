import { useSyncExternalStore, useState } from "react";
import { Search, X } from "lucide-react";
import type { ScriptRecord } from "../../api";
import { getState, loadScripts, setScriptFilter, subscribe } from "./scriptStore";

export function ScriptPopover({
  modelId,
  open,
  onOpenChange,
  onPick,
}: {
  modelId: number;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPick: (script: ScriptRecord) => void;
}) {
  const store = useSyncExternalStore(subscribe, getState, getState);
  const [text, setText] = useState(store.scriptParams.text);

  async function reload(page: number) {
    await loadScripts(modelId, page);
  }

  return (
    <div className="relative">
      <button
        type="button"
        className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm font-medium text-blue-700 shadow-sm hover:bg-blue-50"
        aria-expanded={open}
        onClick={() => {
          const next = !open;
          onOpenChange(next);
          if (next) {
            void reload(1);
          }
        }}
      >
        Scripts
      </button>
      {open ? (
        <div className="absolute left-0 top-full z-[998] mt-2 flex max-h-80 w-72 flex-col overflow-hidden rounded-xl border border-slate-200 bg-white shadow-sm">
          <div className="flex items-center gap-2 border-b border-slate-200 px-3 py-2">
            <Search size={14} className="text-slate-400" />
            <input
              value={text}
              placeholder="Buscar por nombre o descripción"
              className="min-w-0 flex-1 bg-transparent text-sm text-slate-800 outline-none"
              onChange={(event) => {
                const value = event.target.value;
                setText(value);
                setScriptFilter({ text: value });
                void reload(1);
              }}
            />
            <button type="button" aria-label="Cerrar scripts" onClick={() => onOpenChange(false)}>
              <X size={14} className="text-slate-400" />
            </button>
          </div>
          <div className="flex gap-1 overflow-x-auto px-3 py-2">
            <TagButton
              active={!store.activeTab}
              label="Todos"
              onClick={() => {
                setScriptFilter({ tagId: "" });
                void reload(1);
              }}
            />
            {store.tabList.map((tag) => (
              <TagButton
                key={tag}
                active={store.activeTab === tag}
                label={tag}
                onClick={() => {
                  setScriptFilter({ tagId: tag });
                  void reload(1);
                }}
              />
            ))}
          </div>
          <div className="min-h-0 flex-1 overflow-auto px-2 pb-2">
            {store.loading && store.scriptList.length === 0 ? (
              <p className="px-2 py-6 text-center text-sm text-slate-500">Cargando scripts…</p>
            ) : null}
            {store.isError ? (
              <p className="px-2 py-6 text-center text-sm text-orange-700">No se pudieron cargar los scripts.</p>
            ) : null}
            {!store.loading && store.itemsEmpty ? (
              <p className="px-2 py-6 text-center text-sm text-slate-500">
                {store.scriptParams.text || store.scriptParams.tagId
                  ? "Ningún script coincide con el filtro."
                  : "No hay scripts para esta modelo."}
              </p>
            ) : null}
            {store.scriptList.map((script) => (
              <button
                key={script.id}
                type="button"
                className="block w-full rounded-lg px-2 py-2 text-left hover:bg-blue-50"
                onClick={() => {
                  onPick(script);
                  onOpenChange(false);
                }}
              >
                <span className="block text-sm font-medium text-slate-900">{script.scriptName || "Sin nombre"}</span>
                <span className="line-clamp-2 text-xs text-slate-500">{script.scriptTextContent}</span>
              </button>
            ))}
            {store.hasNext ? (
              <button
                type="button"
                className="mt-1 w-full rounded-lg px-2 py-2 text-sm font-medium text-blue-700 hover:bg-blue-50"
                disabled={store.loading}
                onClick={() => void reload(store.scriptParams.page + 1)}
              >
                Cargar más
              </button>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function TagButton({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`shrink-0 rounded-full px-2.5 py-1 text-xs font-medium ${
        active ? "bg-blue-600 text-white" : "bg-slate-100 text-slate-600"
      }`}
      onClick={onClick}
    >
      {label}
    </button>
  );
}
