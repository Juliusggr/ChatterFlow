import { UserPlus } from "lucide-react";
import { useState, type FormEvent } from "react";
import {
  addEmployee,
  deleteEmployee,
  errorMessage,
  toggleClockIn,
  updateEmployeeRoleSchedule,
} from "./api";
import {
  blankSchedule,
  formatStamp,
  ROLES,
  scheduleFromJson,
  summarizeSchedule,
  WEEKDAYS,
  type Employee,
  type EmployeeRole,
  type ScheduleDay,
} from "./types";

type Draft = {
  name: string;
  role: EmployeeRole | "";
  days: ScheduleDay[];
};

function emptyDraft(): Draft {
  return { name: "", role: "", days: blankSchedule() };
}

export function TeamsView({
  employees,
  onChanged,
}: {
  employees: Employee[];
  onChanged: () => Promise<void>;
}) {
  const [draft, setDraft] = useState<Draft>(emptyDraft);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<number | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  function updateDay(index: number, patch: Partial<ScheduleDay>) {
    setDraft((current) => ({
      ...current,
      days: current.days.map((day, dayIndex) => {
        if (dayIndex !== index) return day;
        const next = { ...day, ...patch };
        if (patch.enabled === false) { next.start = ""; next.end = ""; }
        return next;
      }),
    }));
  }

  function beginEdit(employee: Employee) {
    setEditingId(employee.id);
    setPendingDeleteId(null);
    setFormError(null);
    setDraft({
      name: employee.name,
      role: employee.role,
      days: scheduleFromJson(employee.schedule_json),
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
    if (problem) { setFormError(problem); return; }
    if (!draft.role) { setFormError("Selecciona un rol."); return; }
    setBusy(editingId == null ? "save" : `save:${editingId}`);
    setFormError(null);
    try {
      const scheduleJson = { days: draft.days };
      if (editingId == null) {
        await addEmployee({ name: draft.name, role: draft.role, scheduleJson });
      } else {
        await updateEmployeeRoleSchedule({ id: editingId, role: draft.role, scheduleJson });
      }
      setDraft(emptyDraft());
      setEditingId(null);
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function clock(employee: Employee) {
    setBusy(`clock:${employee.id}`);
    setFormError(null);
    try {
      await toggleClockIn(employee.id);
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  async function remove(id: number) {
    setBusy(`delete:${id}`);
    setFormError(null);
    try {
      await deleteEmployee(id);
      if (editingId === id) cancelEdit();
      setPendingDeleteId(null);
      await onChanged();
    } catch (caught) {
      setFormError(errorMessage(caught));
    } finally {
      setBusy(null);
    }
  }

  const saving = busy === "save" || (editingId != null && busy === `save:${editingId}`);

  return (
    <section className="flex flex-col gap-6">
      {/* Header */}
      <header>
        <h1 className="text-2xl font-semibold tracking-tight" style={{ color: "var(--text-primary)" }}>
          Equipos y Roles
        </h1>
        <p className="mt-1 max-w-2xl text-sm" style={{ color: "var(--text-secondary)" }}>
          Registra a quien trabaja la cuenta, asigna su rol y define el horario. Clock in abre el
          turno y Clock out lo cierra.
        </p>
      </header>

      {/* Form card */}
      <form
        onSubmit={(event) => void submit(event)}
        className="cf-card"
        style={{ padding: "1.5rem" }}
      >
        <h2 className="text-base font-semibold flex items-center gap-2" style={{ color: "var(--text-primary)" }}>
          <UserPlus size={16} strokeWidth={2} style={{ color: "var(--brand-500)" }} />
          {editingId == null ? "Agregar empleado" : "Editar rol y horario"}
        </h2>

        <div className="mt-5 grid gap-4 sm:grid-cols-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
              Nombre
            </span>
            <input
              value={draft.name}
              readOnly={editingId != null}
              required={editingId == null}
              autoComplete="name"
              className="cf-input"
              onChange={(event) =>
                setDraft((current) => ({ ...current, name: event.target.value }))
              }
            />
          </label>

          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-medium" style={{ color: "var(--text-secondary)" }}>
              Rol
            </span>
            <select
              value={draft.role}
              required
              className="cf-input"
              style={{ cursor: "pointer" }}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  role: event.target.value as EmployeeRole | "",
                }))
              }
            >
              <option value="">Selecciona un rol</option>
              {ROLES.map((role) => (
                <option key={role} value={role}>
                  {role}
                </option>
              ))}
            </select>
          </label>
        </div>

        {/* Schedule */}
        <fieldset className="mt-6">
          <legend className="text-xs font-medium mb-3" style={{ color: "var(--text-secondary)" }}>
            Horario semanal
          </legend>
          <div className="grid gap-2 sm:grid-cols-2">
            {draft.days.map((day, index) => {
              const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
              return (
                <div
                  key={day.day}
                  className="grid grid-cols-[auto_5.5rem_1fr_1fr] items-center gap-2 rounded-lg px-3 py-2 transition-colors"
                  style={{
                    border: "1px solid var(--border)",
                    background: day.enabled ? "var(--bg-elevated)" : "var(--bg-subtle)",
                  }}
                >
                  <input
                    type="checkbox"
                    checked={day.enabled}
                    aria-label={`${label} trabaja`}
                    className="rounded"
                    onChange={(event) => updateDay(index, { enabled: event.target.checked })}
                  />
                  <span
                    className="text-sm font-medium"
                    style={{ color: day.enabled ? "var(--text-primary)" : "var(--text-disabled)" }}
                  >
                    {label}
                  </span>
                  <input
                    type="time"
                    value={day.start}
                    disabled={!day.enabled}
                    aria-label={`Inicio ${label}`}
                    className="cf-input min-w-0 tabular-nums"
                    style={{ padding: "0.25rem 0.5rem", fontSize: "0.8125rem" }}
                    onChange={(event) => updateDay(index, { start: event.target.value })}
                  />
                  <input
                    type="time"
                    value={day.end}
                    disabled={!day.enabled}
                    aria-label={`Fin ${label}`}
                    className="cf-input min-w-0 tabular-nums"
                    style={{ padding: "0.25rem 0.5rem", fontSize: "0.8125rem" }}
                    onChange={(event) => updateDay(index, { end: event.target.value })}
                  />
                </div>
              );
            })}
          </div>
        </fieldset>

        {formError ? (
          <p
            role="alert"
            className="mt-4 rounded-lg px-3 py-2 text-sm"
            style={{ background: "#fef2f2", color: "#b91c1c", border: "1px solid #fecaca" }}
          >
            {formError}
          </p>
        ) : null}

        <div className="mt-5 flex flex-wrap gap-2">
          <button type="submit" disabled={saving} className="cf-btn-primary">
            {editingId == null ? "Agregar empleado" : "Guardar horario"}
          </button>
          {editingId != null ? (
            <button type="button" className="cf-btn-secondary" onClick={cancelEdit}>
              Cancelar
            </button>
          ) : null}
        </div>
      </form>

      {/* Employees table */}
      <div className="cf-card overflow-hidden">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[52rem] text-left text-sm">
            <caption className="sr-only">Empleados registrados</caption>
            <thead style={{ borderBottom: "1px solid var(--border)" }}>
              <tr>
                {["Nombre", "Rol", "Horario", "Turno", "Último clock in", "Acciones"].map((h) => (
                  <th
                    key={h}
                    className="px-5 py-3.5 text-xs font-semibold uppercase tracking-wide"
                    style={{ color: "var(--text-tertiary)" }}
                  >
                    {h}
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
                employees.map((employee) => {
                  const removing = busy === `delete:${employee.id}`;
                  const clocking = busy === `clock:${employee.id}`;
                  return (
                    <tr
                      key={employee.id}
                      style={{ borderBottom: "1px solid var(--border-muted)" }}
                    >
                      <th
                        scope="row"
                        className="px-5 py-4 font-semibold text-left"
                        style={{ color: "var(--text-primary)" }}
                      >
                        {employee.name}
                      </th>
                      <td className="px-5 py-4">
                        <span className="cf-badge cf-badge-brand">{employee.role}</span>
                      </td>
                      <td className="px-5 py-4 text-xs" style={{ color: "var(--text-secondary)" }}>
                        {summarizeSchedule(scheduleFromJson(employee.schedule_json))}
                      </td>
                      <td className="px-5 py-4">
                        {employee.is_clocked_in ? (
                          <span className="inline-flex items-center gap-1.5 text-sm font-medium" style={{ color: "var(--live)" }}>
                            <span className="h-1.5 w-1.5 rounded-full bg-orange-400 animate-pulse" aria-hidden="true" />
                            Clocked in
                          </span>
                        ) : (
                          <span className="text-sm" style={{ color: "var(--text-disabled)" }}>
                            Clocked out
                          </span>
                        )}
                      </td>
                      <td className="px-5 py-4 tabular-nums text-xs" style={{ color: "var(--text-tertiary)" }}>
                        {formatStamp(employee.last_clock_in)}
                      </td>
                      <td className="px-5 py-4">
                        <div className="flex flex-wrap gap-2">
                          <button
                            type="button"
                            disabled={busy !== null}
                            aria-pressed={employee.is_clocked_in}
                            className={employee.is_clocked_in ? "cf-btn-secondary" : "cf-btn-live"}
                            style={{ padding: "0.375rem 0.75rem" }}
                            onClick={() => void clock(employee)}
                          >
                            {clocking
                              ? "…"
                              : employee.is_clocked_in
                              ? "Clock out"
                              : "Clock in"}
                          </button>
                          <button
                            type="button"
                            className="cf-btn-secondary"
                            style={{ padding: "0.375rem 0.75rem" }}
                            onClick={() => beginEdit(employee)}
                          >
                            Editar
                          </button>
                          {pendingDeleteId === employee.id ? (
                            <>
                              <button
                                type="button"
                                disabled={removing}
                                className="cf-btn-danger"
                                style={{ padding: "0.375rem 0.75rem" }}
                                onClick={() => void remove(employee.id)}
                              >
                                Confirmar
                              </button>
                              <button
                                type="button"
                                className="cf-btn-secondary"
                                style={{ padding: "0.375rem 0.75rem" }}
                                onClick={() => setPendingDeleteId(null)}
                              >
                                Cancelar
                              </button>
                            </>
                          ) : (
                            <button
                              type="button"
                              className="cf-btn-secondary"
                              style={{ padding: "0.375rem 0.75rem", color: "var(--text-tertiary)" }}
                              onClick={() => setPendingDeleteId(employee.id)}
                            >
                              Eliminar
                            </button>
                          )}
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
  if (!editing && draft.name.trim() === "") {
    return "El nombre del empleado es obligatorio.";
  }
  if (!draft.role) return "Selecciona un rol.";
  for (const day of draft.days) {
    if (!day.enabled) continue;
    const label = WEEKDAYS.find((item) => item.day === day.day)?.label ?? day.day;
    if (!day.start || !day.end) return `Indica inicio y fin para ${label}.`;
    if (day.start >= day.end) return `La hora de inicio de ${label} debe ser anterior a la de fin.`;
  }
  return null;
}
