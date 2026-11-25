import { Download, FolderOpen, Image as ImageIcon, Video } from "lucide-react";
import { useState } from "react";
import {
  downloadVaultMedia,
  errorMessage,
  listVaultMedia,
  openModelSession,
  type VaultItem,
} from "./api";
import { avatarColor, formatStamp, initials, modelLabel, type Model } from "./types";

type KindFilter = "all" | "photo" | "video";

export function VaultView({ models }: { models: Model[] }) {
  const [modelId, setModelId] = useState<number | null>(models[0]?.id ?? null);
  const [kind, setKind] = useState<KindFilter>("all");
  const [items, setItems] = useState<VaultItem[]>([]);
  const [offset, setOffset] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const model = models.find((item) => item.id === modelId) ?? null;

  async function load(nextOffset: number, replace: boolean) {
    if (modelId == null) return;
    setBusy("load");
    setFormError(null);
    setNotice(null);
    try {
      const page = await listVaultMedia(modelId, nextOffset, kind);
      setItems((current) => (replace ? page.items : [...current, ...page.items]));
      setOffset(page.nextOffset);
      setHasMore(page.hasMore);
      if (replace) setSelected([]);
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  function toggle(id: string) {
    setSelected((current) => (current.includes(id) ? current.filter((item) => item !== id) : [...current, id]));
  }

  async function download(list: VaultItem[]) {
    if (modelId == null) return;
    const files = list
      .filter((item) => item.downloadUrl)
      .map((item) => ({ id: item.id, kind: item.kind, url: item.downloadUrl as string }));
    const missing = list.length - files.length;
    if (files.length === 0) {
      setFormError("OnlyFans no entregó el archivo original de esos medios.");
      return;
    }
    setBusy("download");
    setFormError(null);
    setNotice(null);
    try {
      const report = await downloadVaultMedia(modelId, files);
      const skipped = missing > 0 ? ` ${missing} sin archivo original.` : "";
      const failures = report.failed.length > 0 ? ` ${report.failed.length} fallaron.` : "";
      setNotice(`Se guardaron ${report.saved} archivos en ${report.directory}.${skipped}${failures}`);
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  if (models.length === 0) {
    return (
      <section className="flex flex-col gap-5">
        <Header />
        <div className="cf-card flex max-w-sm flex-col items-center justify-center gap-3 py-16 text-center">
          <ImageIcon size={32} strokeWidth={1.25} style={{ color: "var(--text-disabled)" }} />
          <p className="text-sm font-medium" style={{ color: "var(--text-secondary)" }}>
            No hay modelos en SQLite.
          </p>
        </div>
      </section>
    );
  }

  return (
    <section className="flex flex-col gap-5">
      <Header />
      <div className="flex flex-wrap items-center gap-2">
        {models.map((item) => {
          const label = modelLabel(item);
          const active = item.id === modelId;
          return (
            <button
              key={item.id}
              type="button"
              className="flex items-center gap-2 rounded-full px-3 py-1.5 text-sm"
              style={{
                background: active ? "var(--brand)" : "var(--bg-subtle)",
                color: active ? "#fff" : "var(--text-primary)",
              }}
              onClick={() => {
                setModelId(item.id);
                setItems([]);
                setSelected([]);
                setHasMore(false);
              }}
            >
              <span
                className="flex h-6 w-6 items-center justify-center rounded-full text-[10px] font-semibold text-white"
                style={{ backgroundColor: avatarColor(item.id) }}
              >
                {initials(label)}
              </span>
              {label}
            </button>
          );
        })}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {(
          [
            ["all", "Todo"],
            ["photo", "Fotos"],
            ["video", "Videos"],
          ] as const
        ).map(([value, label]) => (
          <button
            key={value}
            type="button"
            className="rounded-lg px-3 py-1.5 text-sm"
            style={{
              background: kind === value ? "var(--bg-subtle)" : "transparent",
              color: "var(--text-primary)",
              border: "1px solid var(--border)",
            }}
            onClick={() => setKind(value)}
          >
            {label}
          </button>
        ))}
        <button type="button" className="cf-btn-primary" disabled={busy != null || model == null} onClick={() => void load(0, true)}>
          {busy === "load" ? "Cargando…" : "Cargar vault"}
        </button>
        {model ? (
          <button
            type="button"
            className="cf-btn-secondary inline-flex items-center gap-2"
            onClick={() => void openModelSession(model.id)}
          >
            <FolderOpen size={14} />
            Abrir sesión
          </button>
        ) : null}
      </div>
      {formError ? (
        <p role="alert" className="text-sm" style={{ color: "#b91c1c" }}>
          {formError}
        </p>
      ) : null}
      {notice ? (
        <p className="text-sm" style={{ color: "var(--text-secondary)" }}>
          {notice}
        </p>
      ) : null}
      {items.length > 0 ? (
        <>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="cf-btn-primary inline-flex items-center gap-2"
              disabled={busy != null || selected.length === 0}
              onClick={() => void download(items.filter((item) => selected.includes(item.id)))}
            >
              <Download size={14} />
              Descargar selección ({selected.length})
            </button>
            <button
              type="button"
              className="cf-btn-secondary inline-flex items-center gap-2"
              disabled={busy != null}
              onClick={() => void download(items)}
            >
              <Download size={14} />
              Descargar página
            </button>
            {hasMore ? (
              <button type="button" className="cf-btn-secondary" disabled={busy != null} onClick={() => void load(offset, false)}>
                Cargar más
              </button>
            ) : null}
          </div>
          <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {items.map((item) => {
              const checked = selected.includes(item.id);
              const size = item.width && item.height ? `${item.width}×${item.height}` : "calidad original";
              return (
                <li key={item.id}>
                  <label className="cf-card flex cursor-pointer items-start gap-3 p-3">
                    <input type="checkbox" checked={checked} onChange={() => toggle(item.id)} />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-2 text-sm font-semibold" style={{ color: "var(--text-primary)" }}>
                        {item.kind === "video" ? <Video size={14} /> : <ImageIcon size={14} />}
                        {item.kind === "video" ? "Video" : "Foto"} {item.id}
                      </span>
                      <span className="mt-1 block text-xs" style={{ color: "var(--text-tertiary)" }}>
                        {size}
                        {item.createdAt ? ` · ${formatStamp(item.createdAt)}` : ""}
                      </span>
                      {item.downloadUrl ? null : (
                        <span className="mt-1 block text-xs" style={{ color: "#b45309" }}>
                          Sin archivo original
                        </span>
                      )}
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
        </>
      ) : null}
    </section>
  );
}

function Header() {
  return (
    <header>
      <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
        Vault
      </h1>
      <p className="mt-1 max-w-2xl text-sm" style={{ color: "var(--text-secondary)" }}>
        Descarga las fotos y los videos del vault de la modelo en el archivo de mayor calidad que OnlyFans entrega.
        Abre su sesión antes de cargar el vault.
      </p>
    </header>
  );
}
