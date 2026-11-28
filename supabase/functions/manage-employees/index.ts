import "jsr:@supabase/functions-js/edge-runtime.d.ts";
import { createClient } from "jsr:@supabase/supabase-js@2";

const cors = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Headers": "authorization, x-client-info, apikey, content-type",
};

const ROLES = new Set(["owner", "admin", "chatter", "dropper"]);

type Body = {
  action?: "create" | "update" | "delete";
  userId?: string;
  username?: string;
  password?: string;
  fullName?: string;
  role?: string;
  isActive?: boolean;
};

Deno.serve(async (req) => {
  if (req.method === "OPTIONS") {
    return new Response("ok", { headers: cors });
  }

  try {
    const url = Deno.env.get("SUPABASE_URL") ?? "";
    const anonKey = Deno.env.get("SUPABASE_ANON_KEY") ?? "";
    const serviceKey = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY") ?? "";
    const authHeader = req.headers.get("Authorization") ?? "";
    const caller = createClient(url, anonKey, {
      global: { headers: { Authorization: authHeader } },
    });
    const { data: userData, error: userError } = await caller.auth.getUser();
    if (userError || !userData.user) {
      return json({ error: "No autorizado" }, 401);
    }

    const admin = createClient(url, serviceKey);
    const { data: manager } = await admin
      .from("profiles")
      .select("role,is_active")
      .eq("id", userData.user.id)
      .single();
    if (!manager?.is_active || (manager.role !== "owner" && manager.role !== "admin")) {
      return json({ error: "Solo owner y admin pueden gestionar empleados" }, 403);
    }

    const body = (await req.json()) as Body;
    if (body.action === "create") {
      const username = (body.username ?? "").trim().toLowerCase();
      const role = body.role ?? "";
      const password = body.password ?? "";
      const fullName = (body.fullName ?? username).trim();
      if (!/^[a-z0-9_]{3,32}$/.test(username)) {
        return json({ error: "El usuario debe tener 3-32 letras, números o _" }, 400);
      }
      if (!ROLES.has(role)) {
        return json({ error: "Rol inválido" }, 400);
      }
      if (password.length < 6) {
        return json({ error: "La contraseña debe tener al menos 6 caracteres" }, 400);
      }
      const { data, error } = await admin.auth.admin.createUser({
        email: `${username}@creatorfloww.local`,
        password,
        email_confirm: true,
        app_metadata: {
          provider: "email",
          providers: ["email"],
          username,
          role,
          full_name: fullName,
        },
      });
      if (error || !data.user) {
        return json({ error: error?.message ?? "No se pudo crear la cuenta" }, 400);
      }
      return json({ id: data.user.id }, 200);
    }

    if (body.action === "update") {
      if (!body.userId) {
        return json({ error: "Falta el empleado" }, 400);
      }
      const role = body.role ?? "";
      if (!ROLES.has(role)) {
        return json({ error: "Rol inválido" }, 400);
      }
      const { data: existing, error: readError } = await admin.auth.admin.getUserById(body.userId);
      if (readError || !existing.user) {
        return json({ error: "Empleado no encontrado" }, 404);
      }
      const fullName = (body.fullName ?? String(existing.user.app_metadata?.full_name ?? "")).trim();
      const { error } = await admin.auth.admin.updateUserById(body.userId, {
        app_metadata: {
          ...existing.user.app_metadata,
          role,
          full_name: fullName,
        },
      });
      if (error) {
        return json({ error: error.message }, 400);
      }
      const { error: profileError } = await admin
        .from("profiles")
        .update({
          full_name: fullName,
          role,
          is_active: body.isActive !== false,
        })
        .eq("id", body.userId);
      if (profileError) {
        return json({ error: profileError.message }, 400);
      }
      if (body.isActive === false) {
        const nowIso = new Date().toISOString();
        await admin
          .from("shifts")
          .update({ clock_out: nowIso, status: "completed", total_minutes: 0 })
          .eq("user_id", body.userId)
          .eq("status", "active");
      }
      return json({ id: body.userId }, 200);
    }

    if (body.action === "delete") {
      if (!body.userId) {
        return json({ error: "Falta el empleado" }, 400);
      }
      if (body.userId === userData.user.id) {
        return json({ error: "No puedes eliminar tu propia cuenta activa" }, 400);
      }
      const { data: targetProfile } = await admin
        .from("profiles")
        .select("role,is_active")
        .eq("id", body.userId)
        .maybeSingle();
      if (targetProfile?.role === "owner" && manager.role !== "owner") {
        return json({ error: "Solo un owner puede eliminar a otro owner" }, 403);
      }
      const { error: deleteProfileError } = await admin
        .from("profiles")
        .delete()
        .eq("id", body.userId);
      if (deleteProfileError) {
        return json({ error: deleteProfileError.message }, 400);
      }
      const { error: deleteAuthError } = await admin.auth.admin.deleteUser(body.userId);
      if (deleteAuthError && !/not found/i.test(deleteAuthError.message)) {
        return json({ error: deleteAuthError.message }, 400);
      }
      return json({ id: body.userId }, 200);
    }

    return json({ error: "Acción desconocida" }, 400);
  } catch (error) {
    const message = error instanceof Error ? error.message : "Error interno";
    return json({ error: message }, 500);
  }
});

function json(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...cors, "Content-Type": "application/json" },
  });
}
