-- A weekly slot is only the template of a real check-in. A sale counts when it
-- falls inside that check-in's own window (21:00–04:00 starting the evening they
-- clocked in), not merely because the sale and the check-in share a calendar day.
-- A sale at 00:21 belongs to the overnight window that started the previous
-- evening. It does not belong to the shift that starts at 21:00 later that day.

create or replace function private.clock_in_schedule_window(
  p_user uuid,
  p_clock_in timestamptz,
  p_tz text default 'America/Caracas'
)
returns table(window_start timestamptz, window_end timestamptz)
language plpgsql
stable
security definer
set search_path = ''
as $$
declare
  v_tz text := 'America/Caracas';
  v_local timestamp without time zone;
  v_date date;
  v_dow int;
  v_prev_dow int;
  v_minutes int;
  v_start_min int;
  v_end_min int;
  v_slot_day int;
  v_slot_start time;
  v_slot_end time;
begin
  if p_clock_in is null then
    return;
  end if;
  if p_tz is not null and exists (select 1 from pg_timezone_names where name = p_tz) then
    v_tz := p_tz;
  end if;

  v_local := p_clock_in at time zone v_tz;
  v_date := v_local::date;
  v_dow := (extract(dow from v_local)::int + 6) % 7;
  v_prev_dow := (v_dow + 6) % 7;
  v_minutes := floor(extract(hour from v_local))::int * 60
    + floor(extract(minute from v_local))::int;

  select sched.day_of_week, sched.start_time, sched.end_time
    into v_slot_day, v_slot_start, v_slot_end
  from public.schedules as sched
  where sched.user_id = p_user
    and sched.start_time <> sched.end_time
    and (
      (
        sched.start_time < sched.end_time
        and sched.day_of_week = v_dow
        and v_minutes >= (
          extract(hour from sched.start_time)::int * 60
          + extract(minute from sched.start_time)::int
        )
        and v_minutes < (
          extract(hour from sched.end_time)::int * 60
          + extract(minute from sched.end_time)::int
        )
      )
      or (
        sched.start_time > sched.end_time
        and sched.day_of_week = v_dow
        and v_minutes >= (
          extract(hour from sched.start_time)::int * 60
          + extract(minute from sched.start_time)::int
        )
      )
      or (
        sched.start_time > sched.end_time
        and sched.day_of_week = v_prev_dow
        and v_minutes < (
          extract(hour from sched.end_time)::int * 60
          + extract(minute from sched.end_time)::int
        )
      )
    )
  order by sched.start_time
  limit 1;

  if v_slot_start is null then
    -- Early check-in, up to 90 minutes before the slot starts.
    select sched.day_of_week, sched.start_time, sched.end_time
      into v_slot_day, v_slot_start, v_slot_end
    from public.schedules as sched
    where sched.user_id = p_user
      and sched.day_of_week = v_dow
      and sched.start_time <> sched.end_time
      and (
        extract(hour from sched.start_time)::int * 60
        + extract(minute from sched.start_time)::int
      ) >= v_minutes
      and (
        extract(hour from sched.start_time)::int * 60
        + extract(minute from sched.start_time)::int
        - v_minutes
      ) <= 90
    order by sched.start_time
    limit 1;
  end if;

  if v_slot_start is null then
    return;
  end if;

  v_start_min := extract(hour from v_slot_start)::int * 60 + extract(minute from v_slot_start)::int;
  v_end_min := extract(hour from v_slot_end)::int * 60 + extract(minute from v_slot_end)::int;

  if v_start_min < v_end_min then
    window_start := (v_date + v_slot_start) at time zone v_tz;
    window_end := (v_date + v_slot_end) at time zone v_tz;
  elsif v_slot_day = v_dow then
    window_start := (v_date + v_slot_start) at time zone v_tz;
    window_end := ((v_date + 1) + v_slot_end) at time zone v_tz;
  else
    window_start := ((v_date - 1) + v_slot_start) at time zone v_tz;
    window_end := (v_date + v_slot_end) at time zone v_tz;
  end if;
  return next;
end;
$$;

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
begin
  if p_at is null then
    return false;
  end if;
  if p_tz is not null and exists (select 1 from pg_timezone_names where name = p_tz) then
    v_tz := p_tz;
  end if;

  if (p_at at time zone 'America/Caracas')::date <= date '2026-11-26' then
    return false;
  end if;
  if (p_at at time zone v_tz)::date <= date '2026-11-26' then
    return false;
  end if;

  if not exists (select 1 from public.schedules where user_id = p_user) then
    return private.event_in_clock_shift(p_user, p_at);
  end if;

  return exists (
    select 1
    from public.shifts as shift
    cross join lateral private.clock_in_schedule_window(shift.user_id, shift.clock_in, v_tz) as win
    where shift.user_id = p_user
      and (shift.clock_in at time zone 'America/Caracas')::date >= date '2026-11-27'
      and p_at >= win.window_start
      and p_at < win.window_end
  );
end;
$$;

revoke all on function private.clock_in_schedule_window(uuid, timestamptz, text) from public, anon;
revoke all on function private.chatter_sale_allowed(uuid, timestamptz, text) from public, anon;
grant execute on function private.clock_in_schedule_window(uuid, timestamptz, text) to authenticated, service_role;
grant execute on function private.chatter_sale_allowed(uuid, timestamptz, text) to authenticated, service_role;

delete from public.sales as sale
where not private.chatter_sale_allowed(sale.employee_id, sale.recorded_at, 'America/Caracas');
