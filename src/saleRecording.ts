import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppNotification } from "./api";
import { modelExternalKey, recordSale } from "./agency";
import type { Model } from "./types";

export function recordSaleFromNotification(item: AppNotification, models: Model[]): void {
  if ((item.kind !== "ppv" && item.kind !== "tip") || item.amount == null) return;
  const model = models.find((candidate) => candidate.id === item.modelId);
  if (!model) return;
  const gross = item.amount;
  const net = item.netAmount ?? gross * 0.8;
  void recordSale({
    externalKey: modelExternalKey(model),
    fanPlatformId: item.ofFanId,
    amountGross: gross,
    amountNet: net,
    category: item.kind,
    recordedAt: item.createdAt,
    sourceId: item.id,
  }).catch(() => undefined);
}

export async function startSaleRecording(getModels: () => Model[]): Promise<UnlistenFn> {
  return listen<AppNotification>("cf-notification", (event) => {
    if (!event.payload) return;
    recordSaleFromNotification(event.payload, getModels());
  });
}