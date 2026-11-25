import { useState, type FormEvent } from "react";
import { errorMessage } from "./api";
import { signInWithUsername, type AgencyProfile } from "./agency";

export function LoginScreen({ onSignedIn }: { onSignedIn: (profile: AgencyProfile) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const profile = await signInWithUsername(username, password);
      onSignedIn(profile);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center px-6" style={{ background: "var(--bg)" }}>
      <form onSubmit={(event) => void submit(event)} className="cf-card w-full max-w-sm" style={{ padding: "1.75rem" }}>
        <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
          CreatorFloww
        </h1>
        <p className="mt-1 text-sm" style={{ color: "var(--text-secondary)" }}>
          Entra con tu usuario y contraseña.
        </p>
        <label className="mt-6 flex flex-col gap-1.5">
          <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
            Usuario
          </span>
          <input
            value={username}
            required
            autoComplete="username"
            className="cf-input"
            onChange={(event) => setUsername(event.target.value)}
          />
        </label>
        <label className="mt-4 flex flex-col gap-1.5">
          <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
            Contraseña
          </span>
          <input
            value={password}
            required
            type="password"
            autoComplete="current-password"
            className="cf-input"
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>
        {error ? (
          <p role="alert" className="mt-4 text-sm" style={{ color: "#b91c1c" }}>
            {error}
          </p>
        ) : null}
        <button type="submit" disabled={busy} className="cf-btn-primary mt-6 w-full">
          {busy ? "Entrando…" : "Entrar"}
        </button>
      </form>
    </main>
  );
}
