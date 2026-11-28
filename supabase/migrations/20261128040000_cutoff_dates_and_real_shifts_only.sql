-- Enforce strict agency launch cutoffs and real check-in shifts only:
-- 1. Droppers: NO data on or before 2026-11-25 (only >= 2026-11-26 in America/Caracas).
-- 2. Chatters: NO data on or before 2026-11-26 (only >= 2026-11-27 in America/Caracas).

-- ============================================================================
-- 1. Purge pre-cutoff rows already stored in Supabase
-- ============================================================================

-- Droppers: remove shift reports, shifts, publications, and fan arrivals from 2026-11-25 and earlier
delete from public.dropper_shift_reports
where (shift_start at time zone 'America/Caracas')::date <= date '2026-11-25';

delete from public.shifts as s
using public.profiles as p
where p.id = s.user_id
  and p.role = 'dropper'
  and (s.clock_in at time zone 'America/Caracas')::date <= date '2026-11-25';

delete from public.publications
where (created_at at time zone 'America/Caracas')::date <= date '2026-11-25';

delete from public.fan_arrivals
where (arrived_at at time zone 'America/Caracas')::date <= date '2026-11-25';

-- Chatters: remove sales and chatter shifts from 2026-11-26 and earlier
delete from public.sales
where (recorded_at at time zone 'America/Caracas')::date <= date '2026-11-26';

delete from public.shifts as s
using public.profiles as p
where p.id = s.user_id
  and p.role = 'chatter'
  and (s.clock_in at time zone 'America/Caracas')::date <= date '2026-11-26';

-- ============================================================================
-- 2. Enforce Chatter >= 2026-11-27 in private.chatter_sale_allowed
-- ============================================================================

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

  -- Hard cutoff: no chatter data on or before November 26, 2026.
  if (p_at at time zone 'America/Caracas')::date <= date '2026-11-26' then
    return false;
  end if;

  v_local := p_at at time zone v_tz;
  v_date := v_local::date;
  if v_date <= date '2026-11-26' then
    return false;
  end if;

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
      and (shift.clock_in at time zone 'America/Caracas')::date >= date '2026-11-27'
  ) then
    return true;
  end if;

  -- Overnight slot after midnight belongs to the check-in that started the day before (only if >= 2026-11-27).
  return exists (
    select 1
    from public.shifts as shift
    join public.schedules as sched
      on sched.user_id = shift.user_id
     and sched.start_time > sched.end_time
     and sched.day_of_week = ((extract(dow from (shift.clock_in at time zone v_tz))::int + 6) % 7)
    where shift.user_id = p_user
      and (shift.clock_in at time zone v_tz)::date = v_date - 1
      and (shift.clock_in at time zone 'America/Caracas')::date >= date '2026-11-27'
  );
end;
$$;

revoke all on function private.chatter_sale_allowed(uuid, timestamptz, text) from public, anon;
grant execute on function private.chatter_sale_allowed(uuid, timestamptz, text) to authenticated, service_role;

-- ============================================================================
-- 3. Enforce Dropper >= 2026-11-26 in publication & fan arrival RPCs
-- ============================================================================

create or replace function public.record_publication(
  p_model_id uuid,
  p_of_post_id text,
  p_created_at timestamptz,
  p_author_user_id uuid default null
)
returns boolean
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_post_id text := btrim(coalesce(p_of_post_id, ''));
  v_created timestamptz := coalesce(p_created_at, now());
  v_author uuid := p_author_user_id;
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if v_post_id = '' then
    return false;
  end if;
  if (v_created at time zone 'America/Caracas')::date <= date '2026-11-25' then
    return false;
  end if;

  if v_author is null then
    select id into v_author
    from public.profiles
    where id = (select auth.uid())
      and role = 'dropper'
      and is_active;
  end if;

  insert into public.publications (model_id, of_post_id, created_at, author_user_id)
  values (p_model_id, v_post_id, v_created, v_author)
  on conflict (model_id, of_post_id) do update
    set author_user_id = coalesce(public.publications.author_user_id, excluded.author_user_id)
    where public.publications.author_user_id is null and excluded.author_user_id is not null;

  get diagnostics v_rows = row_count;
  return v_rows > 0;
end;
$$;

create or replace function public.record_publications_batch(
  p_model_id uuid,
  p_items jsonb,
  p_author_user_id uuid default null
)
returns integer
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_author uuid;
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if p_items is null or jsonb_typeof(p_items) <> 'array' or jsonb_array_length(p_items) = 0 then
    return 0;
  end if;

  v_author := p_author_user_id;
  if v_author is null then
    select id into v_author
    from public.profiles
    where id = (select auth.uid())
      and role = 'dropper'
      and is_active;
  end if;

  with parsed as (
    select distinct on (btrim(item ->> 'of_post_id'))
      btrim(item ->> 'of_post_id') as of_post_id,
      coalesce(nullif(btrim(item ->> 'created_at'), '')::timestamptz, now()) as created_at
    from jsonb_array_elements(p_items) as item
    where coalesce(btrim(item ->> 'of_post_id'), '') <> ''
  )
  insert into public.publications (model_id, of_post_id, created_at, author_user_id)
  select p_model_id, parsed.of_post_id, parsed.created_at, v_author
  from parsed
  where (parsed.created_at at time zone 'America/Caracas')::date >= date '2026-11-26'
  on conflict (model_id, of_post_id) do update
    set author_user_id = coalesce(public.publications.author_user_id, excluded.author_user_id)
    where public.publications.author_user_id is null and excluded.author_user_id is not null;

  get diagnostics v_rows = row_count;
  return v_rows;
end;
$$;

create or replace function public.record_fan_arrival(
  p_model_id uuid,
  p_of_fan_id text,
  p_arrived_at timestamptz
)
returns boolean
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_fan_id text := btrim(coalesce(p_of_fan_id, ''));
  v_arrived timestamptz := coalesce(p_arrived_at, now());
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if v_fan_id = '' then
    return false;
  end if;
  if (v_arrived at time zone 'America/Caracas')::date <= date '2026-11-25' then
    return false;
  end if;

  insert into public.fan_arrivals (model_id, of_fan_id, arrived_at)
  values (p_model_id, v_fan_id, v_arrived)
  on conflict (model_id, of_fan_id) do nothing;

  get diagnostics v_rows = row_count;
  return v_rows > 0;
end;
$$;

create or replace function public.record_fan_arrivals_batch(
  p_model_id uuid,
  p_items jsonb
)
returns integer
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if p_items is null or jsonb_typeof(p_items) <> 'array' or jsonb_array_length(p_items) = 0 then
    return 0;
  end if;

  with parsed as (
    select distinct on (btrim(item ->> 'of_fan_id'))
      btrim(item ->> 'of_fan_id') as of_fan_id,
      coalesce(nullif(btrim(item ->> 'arrived_at'), '')::timestamptz, now()) as arrived_at
    from jsonb_array_elements(p_items) as item
    where coalesce(btrim(item ->> 'of_fan_id'), '') <> ''
  )
  insert into public.fan_arrivals (model_id, of_fan_id, arrived_at)
  select p_model_id, parsed.of_fan_id, parsed.arrived_at
  from parsed
  where (parsed.arrived_at at time zone 'America/Caracas')::date >= date '2026-11-26'
  on conflict (model_id, of_fan_id) do nothing;

  get diagnostics v_rows = row_count;
  return v_rows;
end;
$$;

revoke all on function public.record_publication(uuid, text, timestamptz, uuid) from public, anon;
revoke all on function public.record_publications_batch(uuid, jsonb, uuid) from public, anon;
revoke all on function public.record_fan_arrival(uuid, text, timestamptz) from public, anon;
revoke all on function public.record_fan_arrivals_batch(uuid, jsonb) from public, anon;
grant execute on function public.record_publication(uuid, text, timestamptz, uuid) to authenticated;
grant execute on function public.record_publications_batch(uuid, jsonb, uuid) to authenticated;
grant execute on function public.record_fan_arrival(uuid, text, timestamptz) to authenticated;
grant execute on function public.record_fan_arrivals_batch(uuid, jsonb) to authenticated;
