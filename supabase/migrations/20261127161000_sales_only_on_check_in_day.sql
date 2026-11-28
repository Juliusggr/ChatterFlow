-- A weekly schedule (for example 08:00–14:00, Mon–Sat) is only the window of a
-- real check-in. A sale counts when that employee clocked in on the sale's
-- local day and the sale time falls inside the schedule. It does not count on
-- earlier days that merely share the same weekday hours.
-- Employees with no schedule still use the clock-in..clock-out interval.

create or replace function private.chatter_sale_allowed(
  p_user uuid,
  p_at timestamptz,
  p_tz text default 'America/Caracas'
)
returns boolean
language plpgsql
stable
security definer
set search_path = ''
as $$
declare
  v_tz text := 'America/Caracas';
  v_local timestamp without time zone;
  v_date date;
begin
  if p_at is null then
    return false;
  end if;
  if p_tz is not null and exists (select 1 from pg_timezone_names where name = p_tz) then
    v_tz := p_tz;
  end if;
  v_local := p_at at time zone v_tz;
  v_date := v_local::date;

  if not exists (select 1 from public.schedules where user_id = p_user) then
    return private.event_in_clock_shift(p_user, p_at);
  end if;

  if not private.event_in_scheduled_slot(p_user, p_at, v_tz) then
    return false;
  end if;

  if exists (
    select 1
    from public.shifts as shift
    where shift.user_id = p_user
      and (shift.clock_in at time zone v_tz)::date = v_date
  ) then
    return true;
  end if;

  -- Overnight slot after midnight belongs to the check-in that started the day before.
  return exists (
    select 1
    from public.shifts as shift
    join public.schedules as sched
      on sched.user_id = shift.user_id
     and sched.start_time > sched.end_time
     and sched.day_of_week = ((extract(dow from (shift.clock_in at time zone v_tz))::int + 6) % 7)
    where shift.user_id = p_user
      and (shift.clock_in at time zone v_tz)::date = v_date - 1
  );
end;
$$;

revoke all on function private.chatter_sale_allowed(uuid, timestamptz, text) from public, anon;
grant execute on function private.chatter_sale_allowed(uuid, timestamptz, text) to authenticated, service_role;

-- Drop sales that were stored against a weekday schedule without a check-in that day.
delete from public.sales as sale
where not private.chatter_sale_allowed(sale.employee_id, sale.recorded_at, 'America/Caracas');
