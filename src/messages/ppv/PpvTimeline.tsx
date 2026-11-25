import { useEffect, useState } from "react";
import { BarChart3 } from "lucide-react";
import { errorMessage, getPpvTimeline, type PpvTimelineState } from "../../api";

const EMPTY: PpvTimelineState = {
  ppvData: [],
  ppvPrice: null,
  ppvAvgPrice: null,
  page: 1,
  size: 10,
  hasNext: false,
  ppvLoading: false,
  firstLoading: false,
  itemsEmpty: true,
  payCount: 0,
  allPayCount: 0,
  payPercent: 0,
};

export function PpvTimeline({
  modelId,
  ofFanId,
  reloadKey = 0,
}: {
  modelId: number;
  ofFanId: string | null;
  reloadKey?: number;
}) {
  const [state, setState] = useState<PpvTimelineState>(EMPTY);
  const [error, setError] = useState<string | null>(null);
  const [chartOpen, setChartOpen] = useState(false);

  useEffect(() => {
    setChartOpen(false);
    if (!ofFanId) {
      setState(EMPTY);
      setError(null);
      return;
    }
    let alive = true;
    setState((current) => ({ ...current, firstLoading: true, ppvLoading: true }));
    void getPpvTimeline(modelId, ofFanId, 1)
      .then((next) => {
        if (alive) {
          setState(next);
          setError(null);
        }
      })
      .catch((caught) => {
        if (alive) setError(errorMessage(caught));
      });
    return () => {
      alive = false;
    };
  }, [modelId, ofFanId, reloadKey]);

  async function loadMore() {
    if (!ofFanId || !state.hasNext || state.ppvLoading) return;
    setState((current) => ({ ...current, ppvLoading: true }));
    try {
      const next = await getPpvTimeline(modelId, ofFanId, state.page + 1);
      setState((current) => ({
        ...next,
        ppvData: next.page > 1 ? [...current.ppvData, ...next.ppvData] : next.ppvData,
      }));
      setError(null);
    } catch (caught) {
      setError(errorMessage(caught));
      setState((current) => ({ ...current, ppvLoading: false }));
    }
  }

  const percent = Math.round((state.payPercent || 0) * 100);

  return (
    <section className="rounded-xl border border-slate-200 bg-white p-3 shadow-sm">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-semibold text-slate-900">PPV timeline</h2>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-lg bg-blue-600 px-2 py-1 text-xs font-medium text-white"
          disabled={!ofFanId}
          onClick={() => setChartOpen(true)}
        >
          <BarChart3 size={12} />
          See chart
        </button>
      </div>
      {!ofFanId ? <p className="mt-3 text-sm text-slate-500">Selecciona un chat.</p> : null}
      {error ? <p className="mt-3 text-sm text-orange-700">{error}</p> : null}
      {ofFanId && !state.firstLoading ? (
        <p className="mt-2 text-sm text-slate-700">
          Buy rate{" "}
          <span className="font-semibold text-orange-600">
            {state.payCount}/{state.allPayCount} ({percent}%)
          </span>
        </p>
      ) : null}
      {ofFanId && state.itemsEmpty && !state.firstLoading && !error ? (
        <p className="mt-2 text-sm text-slate-500">Sin PPVs registrados.</p>
      ) : null}
      <ul className="mt-2 flex flex-col gap-1">
        {state.ppvData.map((point) => (
          <li key={point.id} className="flex items-center justify-between text-xs text-slate-600">
            <span>{point.createdAt}</span>
            <span>{point.price == null ? "Sin precio" : point.price.toFixed(2)}</span>
            <span className={point.purchased ? "text-blue-700" : "text-slate-400"}>
              {point.purchased ? "Pagado" : "No pagado"}
            </span>
          </li>
        ))}
      </ul>
      {state.hasNext ? (
        <button type="button" className="mt-2 text-xs font-medium text-blue-700" onClick={() => void loadMore()}>
          Cargar más
        </button>
      ) : null}
      {chartOpen ? (
        <div className="mt-3 rounded-lg border border-slate-200 bg-white p-3">
          <div className="mb-2 flex items-center justify-between">
            <p className="text-sm font-medium text-slate-800">Spending habits</p>
            <button type="button" className="text-xs text-slate-500" onClick={() => setChartOpen(false)}>
              Close
            </button>
          </div>
          {state.itemsEmpty ? (
            <p className="text-sm text-slate-500">No hay puntos para el gráfico.</p>
          ) : (
            <p className="text-xs text-slate-500">
              The amount of PPVs the fan purchased over time. Precio medio:{" "}
              {state.ppvAvgPrice == null ? "—" : state.ppvAvgPrice.toFixed(2)}
            </p>
          )}
        </div>
      ) : null}
    </section>
  );
}
