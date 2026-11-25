import type { ScriptRecord, VaultMedia } from "../api";

export const MEDIA_PRICE_MIN = 3;
export const MEDIA_PRICE_MAX = 200;

export type ComposerDraft = {
  scriptContent: string;
  scriptTextContent: string;
  mediaPrice: number | null;
  media: VaultMedia[];
  releaseForms: unknown[];
  lockedText: boolean;
};

export const EMPTY_DRAFT: ComposerDraft = {
  scriptContent: "",
  scriptTextContent: "",
  mediaPrice: null,
  media: [],
  releaseForms: [],
  lockedText: false,
};

export function applyFanName(text: string, nickname: string): string {
  const name = nickname.trim();
  if (!name) {
    return text;
  }
  return text.replaceAll("{name}", name);
}

export function draftFromScript(script: ScriptRecord, nickname: string): ComposerDraft {
  return {
    scriptContent: applyFanName(script.scriptContent, nickname),
    scriptTextContent: applyFanName(script.scriptTextContent, nickname),
    mediaPrice: script.mediaPrice,
    media: script.media,
    releaseForms: script.releaseForms,
    lockedText: script.lockedText,
  };
}
