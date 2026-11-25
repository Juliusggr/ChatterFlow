import { createClient } from "@supabase/supabase-js";

export const SUPABASE_URL = "https://rbkjrektvaifegxusrwt.supabase.co";
export const SUPABASE_PUBLISHABLE_KEY = "sb_publishable_Isv24nfB0NANyUTWQWddmA_doEZBkgn";
export const AUTH_EMAIL_DOMAIN = "creatorfloww.local";

export const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
    detectSessionInUrl: false,
  },
});

export function usernameToEmail(username: string): string {
  return `${username.trim().toLowerCase()}@${AUTH_EMAIL_DOMAIN}`;
}
