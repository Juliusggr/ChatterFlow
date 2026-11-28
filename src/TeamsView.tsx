import { Trash2, UserPlus } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";
import { errorMessage } from "./api";
import {
  deactivateEmployee,
  deleteEmployee,
  listEmployees,
  roleLabel,
  saveEmployee,
  setEmployeeActive,
  type AgencyEmployee,
  type AgencyRole,
} from "./agency";
import { shiftsForRole } from "./shifts";
import { blankSchedule, modelLabel, summarizeSchedule, WEEKDAYS, type Model, type ScheduleDay } from "./types";

const ROLES: AgencyRole[] = ["owner", "admin", "chatter", "dropper"];

type StatusFilter = "all" | "active" | "inactive";

type Draft = {
  username: string;
  password: string;
  name: string;
  role: AgencyRole | "";
  isActive: boolean;
  days: ScheduleDay[];
  modelIds: string[];
};

function emptyDraft(): Draft {
  return {
    username: "",
    password: "",
    name: "",
    role: "",
    isActive: true,
    days: blankSchedule(),
    modelIds: [],
  };
}

export function TeamsView({
  models,
  currentUserId,
  currentUserRole = "owner",
}: {
  models: Model[];
  currentUserId?: string;
  currentUserRole?: AgencyRole;
}) {
  const [employees, setEmployees] = useState<AgencyEmployee[]>([]);
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [draft, setDraft] = useState<Draft>(emptyDraft());
  const [editingId, setEditingId] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<{ id: string; kind: "deactivate" | "delete" } | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  async function reload() {
    const all = await listEmployees();
    all.sort(
      (a, b) =>
        Number(b.is_active) - Number(a.is_active) ||
        a.name.localeCompare(b.name, "es"),
    );
    setEmployees(all);
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
    setPendingAction(null);
    setFormError(null);
    setDraft({
      username: employee.username,
      password: "",
      name: employee.name,
      role: employee.role,
      isActive: employee.is_active,
      days: employee.days,
      modelIds: employee.modelIds,
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
        isActive: editingId == null ? true : draft.isActive,
        days: draft.days,
        modelIds: draft.modelIds,
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

  async function deactivate(id: string) {
    setBusy(`deactivate:${id}`);
    setFormError(null);
    try {
      await deactivateEmployee(id);
      if (editingId === id) cancelEdit();
      setPendingAction(null);
      await reload();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function reactivate(id: string) {
    setBusy(`reactivate:${id}`);
    setFormError(null);
    try {
      await setEmployeeActive(id, true);
      setPendingAction(null);
      await reload();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function permanentlyDelete(id: string) {
    setBusy(`delete:${id}`);
    setFormError(null);
    try {
      await deleteEmployee(id);
      if (editingId === id) cancelEdit();
      setPendingAction(null);
      await reload();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  const saving = busy === "save" || (editingId != null && busy === `save:${editingId}`);
  const assignable = draft.role === "chatter" || draft.role === "dropper";
  const activeCount = employees.filter((e) => e.is_active).length;
  const inactiveCount = employees.length - activeCount;
  const visibleEmployees = employees.filter((employee) => {
    if (statusFilter === "active") return employee.is_active;
    if (statusFilter === "inactive") return !employee.is_active;
    return true;
  });

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
              onChange={(event) => {
                const role = event.target.value as AgencyRole | "";
                setDraft((current) => ({
                  ...current,
                  role,
                  days: current.days.map((day) => {
                    const shifts = shiftsForRole(role);
                    if (!shifts) return day;
                    const match = shifts.some((shift) => shift.start === day.start && shift.end === day.end);
                    if (day.enabled && match) return day;
                    return { ...day, enabled: false, start: "", end: "" };
                  }),
                }));
              }}
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

        <fieldset className="mt-6">
          <legend className="mb-3 text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
            Horario semanal
          </legend>
          <div className="grid gap-2 sm:grid-cols-2">
            {draft.days.map((day, index) => {
              const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
              const shifts = shiftsForRole(draft.role);
              return (
                <div
                  key={day.day}
                  className={`grid items-center gap-2 rounded-lg px-3 py-2 ${shifts ? "grid-cols-[auto_5.5rem_1fr]" : "grid-cols-[auto_5.5rem_1fr_1fr]"}`}
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
                  {shifts ? (
                    <select
                      value={shifts.some((shift) => shift.start === day.start && shift.end === day.end) ? `${day.start}|${day.end}` : ""}
                      disabled={!day.enabled}
                      aria-label={`Turno ${label}`}
                      className="cf-input min-w-0"
                      onChange={(event) => {
                        const [start, end] = event.target.value.split("|");
                        if (!start || !end) return;
                        updateDay(index, { start, end });
                      }}
                    >
                      <option value="" disabled>Elige un turno</option>
                      {(["Droppers", "Chatters"] as const).filter((group) => shifts.some((shift) => shift.group === group)).map((group) => (
                        <optgroup key={group} label={group}>
                          {shifts.filter((shift) => shift.group === group).map((shift) => (
                            <option key={`${shift.start}-${shift.end}`} value={`${shift.start}|${shift.end}`}>
                              {shift.label}
                            </option>
                          ))}
                        </optgroup>
                      ))}
                    </select>
                  ) : (
                    <>
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
                    </>
                  )}
                </div>
              );
            })}
          </div>
        </fieldset>

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
                models.map((model) => (
                  <label key={model.id} className="flex items-center gap-2 text-sm" style={{ color: "var(--text-primary)" }}>
                    <input
                      type="checkbox"
                      checked={draft.modelIds.includes(model.id)}
                      onChange={(event) =>
                        setDraft((current) => ({
                          ...current,
                          modelIds: event.target.checked
                            ? [...current.modelIds, model.id]
                            : current.modelIds.filter((item) => item !== model.id),
                        }))
                      }
                    />
                    {modelLabel(model)}
                  </label>
                ))
              )}
            </div>
          </fieldset>
        ) : null}

        {formError ? (
          <p role="alert" className="mt-4 text-sm" style={{ color: "var(--danger)" }}>
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
        <div
          className="flex flex-wrap items-center justify-between gap-3 px-5 py-3.5"
          style={{ borderBottom: "1px solid var(--border)" }}
        >
          <span className="text-sm font-medium" style={{ color: "var(--text-primary)" }}>
            Cuentas registradas
          </span>
          <div
            className="inline-flex rounded-lg p-0.5"
            style={{ background: "var(--bg-subtle)", border: "1px solid var(--border)" }}
            role="group"
            aria-label="Filtrar empleados por estado"
          >
            {(
              [
                { id: "all", label: `Todos (${employees.length})` },
                { id: "active", label: `Activos (${activeCount})` },
                { id: "inactive", label: `Desactivados (${inactiveCount})` },
              ] as const
            ).map((tab) => {
              const selected = statusFilter === tab.id;
              return (
                <button
                  key={tab.id}
                  type="button"
                  aria-pressed={selected}
                  className="rounded-md px-2.5 py-1 text-xs font-medium transition-colors"
                  style={{
                    background: selected ? "var(--bg-elevated)" : "transparent",
                    color: selected ? "var(--text-primary)" : "var(--text-secondary)",
                  }}
                  onClick={() => setStatusFilter(tab.id)}
                >
                  {tab.label}
                </button>
              );
            })}
          </div>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-[44rem] text-left text-sm">
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
              {visibleEmployees.length === 0 ? (
                <tr>
                  <td colSpan={6} className="px-5 py-10 text-center text-sm" style={{ color: "var(--text-tertiary)" }}>
                    {statusFilter === "inactive"
                      ? "No hay usuarios desactivados."
                      : "No hay empleados registrados."}
                  </td>
                </tr>
              ) : (
                visibleEmployees.map((employee) => {
                  const isSelf = currentUserId != null && employee.id === currentUserId;
                  const protectedOwner = employee.role === "owner" && currentUserRole !== "owner";
                  const rowBusy =
                    busy === `deactivate:${employee.id}` ||
                    busy === `reactivate:${employee.id}` ||
                    busy === `delete:${employee.id}`;
                  const confirmingDeactivate =
                    pendingAction?.id === employee.id && pendingAction.kind === "deactivate";
                  const confirmingDelete =
                    pendingAction?.id === employee.id && pendingAction.kind === "delete";
                  return (
                    <tr
                      key={employee.id}
                      style={{
                        borderBottom: "1px solid var(--border-muted)",
                        opacity: employee.is_active ? 1 : 0.72,
                      }}
                    >
                      <th scope="row" className="px-5 py-4 text-left font-semibold" style={{ color: "var(--text-primary)" }}>
                        <div className="flex flex-wrap items-center gap-2">
                          <span>{employee.name}</span>
                          {!employee.is_active ? (
                            <span
                              className="rounded-md px-2 py-0.5 text-[11px] font-medium"
                              style={{
                                background: "var(--danger-bg)",
                                color: "var(--danger)",
                                border: "1px solid var(--danger-border)",
                              }}
                            >
                              Desactivado
                            </span>
                          ) : null}
                        </div>
                      </th>
                      <td className="px-5 py-4" style={{ color: "var(--text-secondary)" }}>{employee.username}</td>
                      <td className="px-5 py-4">
                        <span className="cf-badge cf-badge-brand">{roleLabel(employee.role)}</span>
                      </td>
                      <td className="px-5 py-4 text-xs" style={{ color: "var(--text-secondary)" }}>
                        {summarizeSchedule(employee.days)}
                      </td>
                      <td
                        className="px-5 py-4 text-sm"
                        style={{
                          color: !employee.is_active
                            ? "var(--text-disabled)"
                            : employee.is_clocked_in
                              ? "var(--live)"
                              : "var(--text-disabled)",
                        }}
                      >
                        {!employee.is_active ? "Inactivo" : employee.is_clocked_in ? "En turno" : "Fuera"}
                      </td>
                      <td className="px-5 py-4">
                        <div className="flex flex-wrap items-center gap-2">
                          <button
                            type="button"
                            disabled={rowBusy || protectedOwner}
                            className="cf-btn-secondary"
                            style={{ padding: "0.375rem 0.75rem" }}
                            onClick={() => beginEdit(employee)}
                          >
                            Editar
                          </button>
                          {employee.is_active ? (
                            confirmingDeactivate ? (
                              <>
                                <button
                                  type="button"
                                  disabled={rowBusy}
                                  className="cf-btn-danger"
                                  style={{ padding: "0.375rem 0.75rem" }}
                                  onClick={() => void deactivate(employee.id)}
                                >
                                  Confirmar baja
                                </button>
                                <button
                                  type="button"
                                  disabled={rowBusy}
                                  className="cf-btn-secondary"
                                  style={{ padding: "0.375rem 0.75rem" }}
                                  onClick={() => setPendingAction(null)}
                                >
                                  Cancelar
                                </button>
                              </>
                            ) : !isSelf && !protectedOwner ? (
                              <button
                                type="button"
                                disabled={rowBusy}
                                className="cf-btn-secondary"
                                style={{ padding: "0.375rem 0.75rem" }}
                                onClick={() => setPendingAction({ id: employee.id, kind: "deactivate" })}
                              >
                                Desactivar
                              </button>
                            ) : null
                          ) : !protectedOwner ? (
                            <button
                              type="button"
                              disabled={rowBusy}
                              className="cf-btn-primary"
                              style={{ padding: "0.375rem 0.75rem" }}
                              onClick={() => void reactivate(employee.id)}
                            >
                              Reactivar
                            </button>
                          ) : null}

                          {!isSelf && !protectedOwner ? (
                            confirmingDelete ? (
                              <>
                                <button
                                  type="button"
                                  disabled={rowBusy}
                                  className="cf-btn-danger"
                                  style={{ padding: "0.375rem 0.75rem" }}
                                  onClick={() => void permanentlyDelete(employee.id)}
                                >
                                  Confirmar eliminar
                                </button>
                                <button
                                  type="button"
                                  disabled={rowBusy}
                                  className="cf-btn-secondary"
                                  style={{ padding: "0.375rem 0.75rem" }}
                                  onClick={() => setPendingAction(null)}
                                >
                                  Cancelar
                                </button>
                              </>
                            ) : (
                              <button
                                type="button"
                                disabled={rowBusy}
                                className="cf-btn-danger inline-flex items-center gap-1"
                                style={{ padding: "0.375rem 0.75rem" }}
                                title={`Eliminar cuenta de ${employee.name}`}
                                onClick={() => setPendingAction({ id: employee.id, kind: "delete" })}
                              >
                                <Trash2 size={14} strokeWidth={1.9} />
                                Eliminar
                              </button>
                            )
                          ) : null}
                        </div>
                      </td>
                    </tr>
                  );
                })
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
  const shifts = shiftsForRole(draft.role);
  const optionalSchedule = draft.role === "owner" || draft.role === "admin";
  for (const day of draft.days) {
    if (!day.enabled) continue;
    const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
    if (shifts) {
      const match = shifts.some((shift) => shift.start === day.start && shift.end === day.end);
      if (!match && !optionalSchedule) return `Elige un turno para ${label}.`;
      continue;
    }
    if (!day.start || !day.end) {
      if (optionalSchedule) continue;
      return `Indica inicio y fin para ${label}.`;
    }
    if (day.start >= day.end) return `La hora de inicio de ${label} debe ser anterior a la de fin.`;
  }
  return null;
}
