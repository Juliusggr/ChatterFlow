import { listScripts, type ScriptPage, type ScriptRecord } from "../../api";

export type ScriptQuery = {
  page: number;
  size: 10;
  tagId: string;
  text: string;
  ofModelBindId: string;
  fansId: string;
};

export type ScriptStore = {
  scriptList: ScriptRecord[];
  tabList: string[];
  mediasTags: Record<string, unknown>;
  activeTab?: string;
  scriptParams: ScriptQuery;
  loading: boolean;
  hasNext: boolean;
  isLoadingApi: boolean;
  isError: boolean;
  itemsEmpty: boolean;
  mediaPriceMin: number;
  mediaPriceMax: number;
};

const listeners = new Set<() => void>();
let ticket = 0;

let state: ScriptStore = emptyStore();

function emptyStore(): ScriptStore {
  return {
    scriptList: [],
    tabList: [],
    mediasTags: {},
    activeTab: undefined,
    scriptParams: {
      page: 1,
      size: 10,
      tagId: "",
      text: "",
      ofModelBindId: "",
      fansId: "",
    },
    loading: false,
    hasNext: false,
    isLoadingApi: false,
    isError: false,
    itemsEmpty: true,
    mediaPriceMin: 3,
    mediaPriceMax: 200,
  };
}

export function getState(): ScriptStore {
  return state;
}

export function setState(patch: Partial<ScriptStore>): void {
  state = { ...state, ...patch };
  listeners.forEach((listener) => listener());
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function resetScriptStore(ofModelBindId = "", fansId = ""): void {
  ticket += 1;
  state = emptyStore();
  state.scriptParams = { ...state.scriptParams, ofModelBindId, fansId };
  listeners.forEach((listener) => listener());
}

export function setScriptFilter(patch: { text?: string; tagId?: string }): void {
  const current = getState();
  setState({
    activeTab: patch.tagId === undefined ? current.activeTab : patch.tagId || undefined,
    scriptParams: {
      ...current.scriptParams,
      ...patch,
      page: 1,
      size: 10,
    },
  });
}

export async function loadScripts(modelId: number, page: number): Promise<void> {
  const token = ++ticket;
  const params = getState().scriptParams;
  setState({
    loading: true,
    isLoadingApi: true,
    isError: false,
    scriptParams: { ...params, page, size: 10 },
  });
  try {
    const result = await listScripts({
      modelId,
      page,
      text: params.text,
      tagId: params.tagId,
    });
    if (token !== ticket) {
      return;
    }
    const current = getState();
    setState(applyPage(current, result, page));
  } catch {
    if (token !== ticket) {
      return;
    }
    setState({ loading: false, isLoadingApi: false, isError: true });
  }
}

function applyPage(current: ScriptStore, result: ScriptPage, page: number): Partial<ScriptStore> {
  return {
    scriptList: page > 1 ? [...current.scriptList, ...result.scriptList] : result.scriptList,
    tabList: result.tabList,
    hasNext: result.hasNext,
    loading: false,
    isLoadingApi: false,
    isError: false,
    itemsEmpty: result.itemsEmpty,
    mediaPriceMin: result.mediaPriceMin,
    mediaPriceMax: result.mediaPriceMax,
    scriptParams: { ...current.scriptParams, page, size: 10 },
  };
}
