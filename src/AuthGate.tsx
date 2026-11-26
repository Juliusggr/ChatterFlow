import { useCallback, useEffect, useState, type ReactNode } from "react";
import { loadMyProfile, type AgencyProfile } from "./agency";
import { setSupabaseSession } from "./api";
import { LoginScreen } from "./LoginScreen";
import { supabase } from "./supabase";

export function AuthGate({ children }: { children: (profile: AgencyProfile) => ReactNode }) {
  const [profile, setProfile] = useState<AgencyProfile | null>(null);
  const [ready, setReady] = useState(false);

  const syncSessionAndProfile = useCallback(async (accessTokenOverride?: string | null) => {
    const token =
      accessTokenOverride !== undefined
        ? accessTokenOverride
        : (await supabase.auth.getSession()).data.session?.access_token ?? null;
    if (!token) {
      await setSupabaseSession(null, null, null).catch(() => undefined);
      return null;
    }
    const next = await loadMyProfile();
    const active = next?.is_active ? next : null;
    await setSupabaseSession(active ? token : null, active?.id ?? null, active?.role ?? null).catch(
      () => undefined,
    );
    return active;
  }, []);

  useEffect(() => {
    let alive = true;
    void syncSessionAndProfile()
      .then((active) => {
        if (alive) setProfile(active);
      })
      .catch(() => {
        if (alive) setProfile(null);
      })
      .finally(() => {
        if (alive) setReady(true);
      });

    const { data } = supabase.auth.onAuthStateChange((_event, session) => {
      void syncSessionAndProfile(session?.access_token ?? null)
        .then((active) => {
          if (alive) setProfile(active);
        })
        .catch(() => {
          if (alive) setProfile(null);
        });
    });

    return () => {
      alive = false;
      data.subscription.unsubscribe();
    };
  }, [syncSessionAndProfile]);

  const handleSignedIn = useCallback(async (signedIn: AgencyProfile) => {
    const { data } = await supabase.auth.getSession();
    await setSupabaseSession(
      data.session?.access_token ?? null,
      signedIn.id,
      signedIn.role,
    ).catch(() => undefined);
    setProfile(signedIn);
  }, []);

  if (!ready) {
    return <main className="min-h-screen" style={{ background: "var(--bg)" }} />;
  }
  if (!profile) {
    return <LoginScreen onSignedIn={(next) => void handleSignedIn(next)} />;
  }
  return children(profile);
}
