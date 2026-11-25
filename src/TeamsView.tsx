import { UserPlus } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { errorMessage } from "./api";
import {
  deactivateEmployee,
  listEmployees,
  roleLabel,
  saveEmployee,
  type AgencyEmployee,
  type AgencyRole,
} from "./agency";
import { blankSchedule, modelLabel, summarizeSchedule, WEEKDAYS, type Model, type ScheduleDay } from "./types";

const ROLES: AgencyRole[] = ["owner", "admin", "chatter", "dropper"];

type Draft = {
  username: string;
  password: string;
  name: string;
  role: AgencyRole | "";
  days: ScheduleDay[];
  modelKeys: string[];
};

function emptyDraft(): Draft {
  return { username: "", password: "", name: "", role: "", days: blankSchedule(), modelKeys: [] };
}

export function TeamsView({ models }: { models: Model[] }) {
  const [employees, setEmployees] = useState<AgencyEmployee[]>([]);
  const [draft, setDraft] = useState<Draft>(emptyDraft());
  const [editingId, setEditingId] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  async function reload() {
    setEmployees((await listEmployees()).filter((employee) => employee.is_active));
  }

  useEffect(() => {
    void reload().catch((caught) => setFormError(errorMessage(caught)));
  }, []);

  function updateDay(index: number, patch: Partial<ScheduleDay>) {
    setDraft((current) => ({
      ...current,
      days: current.days.map((day, dayIndex) => {
        if (dayIndex !== index) return day;
        const next = { ...day, ...patch };
        if (patch.enabled === false) {
          next.start = "";
          next.end = "";
        }
        return next;
      }),
    }));
  }

  function beginEdit(employee: AgencyEmployee) {
    setEditingId(employee.id);
    setPendingDeleteId(null);
    setFormError(null);
    setDraft({
      username: employee.username,
      password: "",
      name: employee.name,
      role: employee.role,
      days: employee.days,
      modelKeys: employee.modelKeys,
    });
  }

  function cancelEdit() {
    setEditingId(null);
    setDraft(emptyDraft());
    setFormError(null);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const problem = validateDraft(draft, editingId != null);
    if (problem || !draft.role) {
      setFormError(problem ?? "Selecciona un rol.");
      return;
    }
    setBusy(editingId == null ? "save" : `save:${editingId}`);
    setFormError(null);
    try {
      await saveEmployee({
        id: editingId ?? undefined,
        username: draft.username,
        password: draft.password,
        fullName: draft.name,
        role: draft.role,
        isActive: true,
        days: draft.days,
        modelKeys: draft.modelKeys,
      });
      setDraft(emptyDraft());
      setEditingId(null);
      await reload();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function remove(id: string) {
    setBusy(`delete:${id}`);
    setFormError(null);
    try {
      await deactivateEmployee(id);
      if (editingId === id) cancelEdit();
      setPendingDeleteId(null);
      await reload();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  const saving = busy === "save" || (editingId != null && busy === `save:${editingId}`);
  const assignable = draft.role === "chatter" || draft.role === "dropper";

  return (
    <section className="flex flex-col gap-6">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
          Equipos y Roles
        </h1>
        <p className="mt-1 max-w-2xl text-sm" style={{ color: "var(--text-secondary)" }}>
          Crea cuentas con usuario y contraseña sin cerrar tu sesión. El check-in lo hace cada persona desde el Dashboard.
        </p>
      </header>

      <form onSubmit={(event) => void submit(event)} className="cf-card" style={{ padding: "1.5rem" }}>
        <h2 className="flex items-center gap-2 text-base font-semibold" style={{ color: "var(--text-primary)" }}>
          <UserPlus size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          {editingId == null ? "Agregar empleado" : "Editar rol y horario"}
        </h2>
        <div className="mt-5 grid gap-4 sm:grid-cols-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>Usuario</span>
            <input
              value={draft.username}
              readOnly={editingId != null}
              required={editingId == null}
              autoComplete="off"
              className="cf-input"
              onChange={(event) => setDraft((current) => ({ ...current, username: event.target.value }))}
            />
          </label>
          {editingId == null ? (
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>Contraseña</span>
              <input
                value={draft.password}
                required
                type="password"
                autoComplete="new-password"
                className="cf-input"
                onChange={(event) => setDraft((current) => ({ ...current, password: event.target.value }))}
              />
            </label>
          ) : null}
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>Nombre</span>
            <input
              value={draft.name}
              required
              autoComplete="name"
              className="cf-input"
              onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
            />
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>Rol</span>
            <select
              value={draft.role}
              required
              className="cf-input"
              onChange={(event) =>
                setDraft((current) => ({ ...current, role: event.target.value as AgencyRole | "" }))
              }
            >
              <option value="">Selecciona un rol</option>
              {ROLES.map((role) => (
                <option key={role} value={role}>
                  {roleLabel(role)}
                </option>
              ))}
            </select>
          </label>
        </div>

        {draft.role === "owner" || draft.role === "admin" ? null : (
        <fieldset className="mt-6">
          <legend className="mb-3 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
            Horario semanal
          </legend>
          <div className="grid gap-2 sm:grid-cols-2">
            {draft.days.map((day, index) => {
              const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
              return (
                <div
                  key={day.day}
                  className="grid grid-cols-[auto_5.5rem_1fr_1fr] items-center gap-2 rounded-lg px-3 py-2"
                  style={{
                    border: "1px solid var(--border)",
                    background: day.enabled ? "var(--bg-elevated)" : "var(--bg-subtle)",
                  }}
                >
                  <input
                    type="checkbox"
                    checked={day.enabled}
                    aria-label={`${label} trabaja`}
                    onChange={(event) => updateDay(index, { enabled: event.target.checked })}
                  />
                  <span className="text-sm font-medium" style={{ color: day.enabled ? "var(--text-primary)" : "var(--text-disabled)" }}>
                    {label}
                  </span>
                  <input
                    type="time"
                    value={day.start}
                    disabled={!day.enabled}
                    aria-label={`Inicio ${label}`}
                    className="cf-input min-w-0"
                    onChange={(event) => updateDay(index, { start: event.target.value })}
                  />
                  <input
                    type="time"
                    value={day.end}
                    disabled={!day.enabled}
                    aria-label={`Fin ${label}`}
                    className="cf-input min-w-0"
                    onChange={(event) => updateDay(index, { end: event.target.value })}
                  />
                </div>
              );
            })}
          </div>
        </fieldset>
        )}

        {assignable ? (
          <fieldset className="mt-6">
            <legend className="mb-3 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
              Modelos asignadas
            </legend>
            <div className="flex flex-col gap-2">
              {models.length === 0 ? (
                <p className="text-sm" style={{ color: "var(--text-tertiary)" }}>
                  Todavía no hay modelos sincronizadas.
                </p>
              ) : (
                models.map((model) => {
                  const key = `partition:${model.partition_id}`;
                  return (
                    <label key={model.id} className="flex items-center gap-2 text-sm" style={{ color: "var(--text-primary)" }}>
                      <input
                        type="checkbox"
                        checked={draft.modelKeys.includes(key)}
                        onChange={(event) =>
                          setDraft((current) => ({
                            ...current,
                            modelKeys: event.target.checked
                              ? [...current.modelKeys, key]
                              : current.modelKeys.filter((item) => item !== key),
                          }))
                        }
                      />
                      {modelLabel(model)}
                    </label>
                  );
                })
              )}
            </div>
          </fieldset>
        ) : null}

        {formError ? (
          <p role="alert" className="mt-4 text-sm" style={{ color: "#b91c1c" }}>
            {formError}
          </p>
        ) : null}
        <div className="mt-5 flex flex-wrap gap-2">
          <button type="submit" disabled={saving} className="cf-btn-primary">
            {editingId == null ? "Agregar empleado" : "Guardar cambios"}
          </button>
          {editingId != null ? (
            <button type="button" className="cf-btn-secondary" onClick={cancelEdit}>
              Cancelar
            </button>
          ) : null}
        </div>
      </form>

      <div className="cf-card overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[40rem] text-left text-sm">
            <caption className="sr-only">Empleados registrados</caption>
            <thead style={{ borderBottom: "1px solid var(--border)" }}>
              <tr>
                {["Nombre", "Usuario", "Rol", "Horario", "Turno", "Acciones"].map((heading) => (
                  <th key={heading} className="px-5 py-3.5 text-xs font-semibold uppercase tracking-wide" style={{ color: "var(--text-tertiary)" }}>
                    {heading}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {employees.length === 0 ? (
                <tr>
                  <td colSpan={6} className="px-5 py-10 text-center text-sm" style={{ color: "var(--text-tertiary)" }}>
                    No hay empleados registrados.
                  </td>
                </tr>
              ) : (
                employees.map((employee) => (
                  <tr key={employee.id} style={{ borderBottom: "1px solid var(--border-muted)" }}>
                    <th scope="row" className="px-5 py-4 text-left font-semibold" style={{ color: "var(--text-primary)" }}>
                      {employee.name}
                    </th>
                    <td className="px-5 py-4" style={{ color: "var(--text-secondary)" }}>{employee.username}</td>
                    <td className="px-5 py-4">
                      <span className="cf-badge cf-badge-brand">{roleLabel(employee.role)}</span>
                    </td>
                    <td className="px-5 py-4 text-xs" style={{ color: "var(--text-secondary)" }}>
                      {summarizeSchedule(employee.days)}
                    </td>
                    <td className="px-5 py-4 text-sm" style={{ color: employee.is_clocked_in ? "var(--live)" : "var(--text-disabled)" }}>
                      {employee.is_clocked_in ? "En turno" : "Fuera"}
                    </td>
                    <td className="px-5 py-4">
                      <div className="flex flex-wrap gap-2">
                        <button type="button" className="cf-btn-secondary" style={{ padding: "0.375rem 0.75rem" }} onClick={() => beginEdit(employee)}>
                          Editar
                        </button>
                        {pendingDeleteId === employee.id ? (
                          <button type="button" className="cf-btn-danger" style={{ padding: "0.375rem 0.75rem" }} onClick={() => void remove(employee.id)}>
                            Confirmar baja
                          </button>
                        ) : (
                          <button type="button" className="cf-btn-secondary" style={{ padding: "0.375rem 0.75rem" }} onClick={() => setPendingDeleteId(employee.id)}>
                            Desactivar
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function validateDraft(draft: Draft, editing: boolean): string | null {
  if (!editing && !/^[a-z0-9_]{3,32}$/.test(draft.username.trim().toLowerCase())) {
    return "El usuario debe tener 3-32 letras, números o _.";
  }
  if (!editing && draft.password.length < 6) return "La contraseña debe tener al menos 6 caracteres.";
  if (draft.name.trim() === "") return "El nombre es obligatorio.";
  if (!draft.role) return "Selecciona un rol.";
  if (draft.role === "owner" || draft.role === "admin") return null;
  for (const day of draft.days) {
    if (!day.enabled) continue;
    const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
    if (!day.start || !day.end) return `Indica inicio y fin para ${label}.`;
    if (day.start >= day.end) return `La hora de inicio de ${label} debe ser anterior a la de fin.`;
  }
  return null;
}
