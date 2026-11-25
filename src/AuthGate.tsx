import { useEffect, useState, type ReactNode } from "react";
import { loadMyProfile, type AgencyProfile } from "./agency";
import { LoginScreen } from "./LoginScreen";
import { supabase } from "./supabase";

export function AuthGate({ children }: { children: (profile: AgencyProfile) => ReactNode }) {
  const [profile, setProfile] = useState<AgencyProfile | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let alive = true;
    void loadMyProfile()
      .then((next) => {
        if (alive) setProfile(next?.is_active ? next : null);
      })
      .catch(() => {
        if (alive) setProfile(null);
      })
      .finally(() => {
        if (alive) setReady(true);
      });
    const { data } = supabase.auth.onAuthStateChange(() => {
      void loadMyProfile()
        .then((next) => {
          if (alive) setProfile(next?.is_active ? next : null);
        })
        .catch(() => {
          if (alive) setProfile(null);
        });
    });
    return () => {
      alive = false;
      data.subscription.unsubscribe();
    };
  }, []);

  if (!ready) {
    return <main className="min-h-screen" style={{ background: "var(--bg)" }} />;
  }
  if (!profile) {
    return <LoginScreen onSignedIn={setProfile} />;
  }
  return children(profile);
}
