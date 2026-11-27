-- Chatter sales count when: (1) chatter has an active clock-in, and (2) the sale event time
-- falls inside the assigned schedule slot (e.g. 08:00–14:00 even if check-in was at 08:52).
-- Fallback to clock_in..clock_out when the employee has no schedule rows.

create or replace function private.event_in_scheduled_slot(
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
  v_dow int;
  v_prev_dow int;
  v_minutes int;
  v_start_min int;
  v_end_min int;
begin
  if p_tz is not null and exists (select 1 from pg_timezone_names where name = p_tz) then
    v_tz := p_tz;
  end if;
  v_local := (p_at at time zone v_tz);
  v_dow := (extract(dow from v_local)::int + 6) % 7;
  v_prev_dow := (v_dow + 6) % 7;
  v_minutes := floor(extract(hour from v_local))::int * 60
    + floor(extract(minute from v_local))::int;

  return exists (
    select 1
    from public.schedules as sched
    where sched.user_id = p_user
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
  );
end;
$$;

create or replace function private.event_in_clock_shift(
  p_user uuid,
  p_at timestamptz
)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1
    from public.shifts as shift
    where shift.user_id = p_user
      and p_at >= shift.clock_in - interval '60 seconds'
      and p_at <= coalesce(shift.clock_out, now()) + interval '60 seconds'
  );
$$;

create or replace function private.chatter_sale_allowed(
  p_user uuid,
  p_at timestamptz,
  p_tz text default 'America/Caracas'
)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select (select private.has_active_shift(p_user))
    and (
      (select private.event_in_scheduled_slot(p_user, p_at, p_tz))
      or (
        not exists (select 1 from public.schedules where user_id = p_user)
        and (select private.event_in_clock_shift(p_user, p_at))
      )
    );
$$;

revoke all on function private.event_in_scheduled_slot(uuid, timestamptz, text) from public, anon;
revoke all on function private.event_in_clock_shift(uuid, timestamptz) from public, anon;
revoke all on function private.chatter_sale_allowed(uuid, timestamptz, text) from public, anon;
grant execute on function private.event_in_scheduled_slot(uuid, timestamptz, text) to authenticated, service_role;
grant execute on function private.event_in_clock_shift(uuid, timestamptz) to authenticated, service_role;
grant execute on function private.chatter_sale_allowed(uuid, timestamptz, text) to authenticated, service_role;

create or replace function public.record_sale(
  p_external_key text,
  p_fan_platform_id text,
  p_amount_gross numeric,
  p_amount_net numeric,
  p_category text,
  p_recorded_at timestamptz,
  p_source_id text,
  p_timezone text default 'America/Caracas'
)
returns uuid
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_model uuid;
  v_user uuid := (select auth.uid());
  v_role text;
  v_event_at timestamptz := coalesce(p_recorded_at, now());
  v_id uuid;
begin
  if v_user is null or not (select private.is_active_member()) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  select profile.role into v_role
  from public.profiles as profile
  where profile.id = v_user;
  if v_role = 'dropper' then
    raise exception 'Los droppers no registran ventas' using errcode = '42501';
  end if;
  if not (select private.chatter_sale_allowed(v_user, v_event_at, p_timezone)) then
    raise exception 'La venta está fuera del horario del turno' using errcode = '42501';
  end if;
  select id into v_model
  from public.models
  where external_key = p_external_key;
  if v_model is null or not (select private.can_access_model(v_model)) then
    raise exception 'Modelo no asignado' using errcode = '42501';
  end if;
  insert into public.sales (
    model_id,
    employee_id,
    fan_platform_id,
    amount_gross,
    amount_net,
    category,
    recorded_at,
    source_id
  )
  values (
    v_model,
    v_user,
    coalesce(p_fan_platform_id, ''),
    p_amount_gross,
    p_amount_net,
    p_category,
    v_event_at,
    p_source_id
  )
  on conflict (source_id) do nothing
  returning id into v_id;
  return v_id;
end;
$$;