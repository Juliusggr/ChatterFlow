-- Fixes & enhancements:
-- 1. Employee account activation/deactivation & permanent deletion without losing historical sales.
-- 2. Owner shift deletion across both public.shifts and public.dropper_shift_reports (including FORCE RLS cascade
--    and suppression of regenerated scheduled shifts in Dropper Reports).
-- 3. Persistent muted-fan state (public.fans.is_muted) shared across chatters to silence notifications from muted fans.

-- ============================================================================
-- 1. Employee activation, deletion & last-owner delete protection
-- ============================================================================

-- Preserve historical agency revenue when an employee account is permanently deleted.
alter table public.sales
  drop constraint if exists sales_employee_id_fkey;

alter table public.sales
  alter column employee_id drop not null;

alter table public.sales
  add constraint sales_employee_id_fkey
  foreign key (employee_id) references public.profiles (id) on delete set null;

create or replace function private.is_owner()
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1
    from public.profiles
    where id = (select auth.uid())
      and is_active
      and role = 'owner'
  );
$$;

revoke all on function private.is_owner() from public, anon;
grant execute on function private.is_owner() to authenticated, service_role;

create or replace function private.guard_last_owner_delete()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  if old.role = 'owner' and old.is_active then
    if not exists (
      select 1
      from public.profiles
      where id <> old.id
        and role = 'owner'
        and is_active
    ) then
      raise exception 'Debe quedar al menos un owner activo' using errcode = '23514';
    end if;
  end if;
  return old;
end;
$$;

revoke all on function private.guard_last_owner_delete() from public, anon;

drop trigger if exists profiles_guard_last_owner_delete on public.profiles;
create trigger profiles_guard_last_owner_delete
  before delete on public.profiles
  for each row execute function private.guard_last_owner_delete();

create or replace function public.set_employee_active(
  p_user_id uuid,
  p_is_active boolean
)
returns void
language plpgsql
security definer
set search_path = ''
as $$
begin
  if not (select private.is_manager()) then
    raise exception 'Solo owner y admin pueden gestionar empleados' using errcode = '42501';
  end if;
  if p_user_id is null then
    raise exception 'Falta el empleado' using errcode = '22023';
  end if;
  if not coalesce(p_is_active, false) and p_user_id = (select auth.uid()) then
    raise exception 'No puedes desactivar tu propia cuenta' using errcode = '23514';
  end if;
  if exists (
    select 1 from public.profiles where id = p_user_id and role = 'owner'
  ) and not (select private.is_owner()) then
    raise exception 'Solo un owner puede modificar a otro owner' using errcode = '42501';
  end if;

  update public.profiles
  set is_active = coalesce(p_is_active, false)
  where id = p_user_id;

  if not found then
    raise exception 'Empleado no encontrado' using errcode = 'P0002';
  end if;

  update auth.users
  set raw_app_meta_data = coalesce(raw_app_meta_data, '{}'::jsonb)
    || jsonb_build_object('is_active', coalesce(p_is_active, false))
  where id = p_user_id;

  if not coalesce(p_is_active, false) then
    update public.shifts
    set clock_out = greatest(now(), clock_in),
        status = 'completed',
        total_minutes = greatest(
          0,
          floor(extract(epoch from (greatest(now(), clock_in) - clock_in)) / 60)::integer
        )
    where user_id = p_user_id
      and status = 'active';
  end if;
end;
$$;

create or replace function public.delete_employee_account(
  p_user_id uuid
)
returns void
language plpgsql
security definer
set search_path = ''
as $$
begin
  if not (select private.is_manager()) then
    raise exception 'Solo owner y admin pueden eliminar empleados' using errcode = '42501';
  end if;
  if p_user_id is null then
    raise exception 'Falta el empleado' using errcode = '22023';
  end if;
  if p_user_id = (select auth.uid()) then
    raise exception 'No puedes eliminar tu propia cuenta activa' using errcode = '23514';
  end if;
  if exists (
    select 1 from public.profiles where id = p_user_id and role = 'owner'
  ) and not (select private.is_owner()) then
    raise exception 'Solo un owner puede eliminar a otro owner' using errcode = '42501';
  end if;

  -- Deleting from public.profiles runs profiles_guard_last_owner_delete first
  -- and sets public.sales.employee_id = NULL so historical revenue is preserved.
  delete from public.profiles
  where id = p_user_id;

  delete from auth.users
  where id = p_user_id;
end;
$$;

revoke all on function public.set_employee_active(uuid, boolean) from public, anon;
revoke all on function public.delete_employee_account(uuid) from public, anon;
grant execute on function public.set_employee_active(uuid, boolean) to authenticated;
grant execute on function public.delete_employee_account(uuid) to authenticated;

-- ============================================================================
-- 2. Owner shift deletion (FORCE RLS cascade fix + Dropper Shift Record removal)
-- ============================================================================

drop policy if exists dropper_shift_reports_delete_owner on public.dropper_shift_reports;
create policy dropper_shift_reports_delete_owner on public.dropper_shift_reports
  for delete to authenticated
  using ((select private.is_owner()));

grant delete on public.dropper_shift_reports to authenticated;

create table if not exists public.dismissed_dropper_shifts (
  id uuid primary key default gen_random_uuid(),
  dropper_user_id uuid not null references public.profiles (id) on delete cascade,
  shift_key text not null,
  window_start timestamptz,
  window_end timestamptz,
  created_at timestamptz not null default now(),
  constraint dismissed_dropper_shifts_unique unique (dropper_user_id, shift_key)
);

create index if not exists dismissed_dropper_shifts_user_start_idx
  on public.dismissed_dropper_shifts (dropper_user_id, window_start desc);

alter table public.dismissed_dropper_shifts enable row level security;
alter table public.dismissed_dropper_shifts force row level security;

drop policy if exists dismissed_dropper_shifts_select on public.dismissed_dropper_shifts;
create policy dismissed_dropper_shifts_select on public.dismissed_dropper_shifts
  for select to authenticated
  using (
    (select private.is_manager())
    or dropper_user_id = (select auth.uid())
  );

drop policy if exists dismissed_dropper_shifts_owner_write on public.dismissed_dropper_shifts;
create policy dismissed_dropper_shifts_owner_write on public.dismissed_dropper_shifts
  for all to authenticated
  using ((select private.is_owner()))
  with check ((select private.is_owner()));

revoke all on public.dismissed_dropper_shifts from public, anon;
grant select, insert, update, delete on public.dismissed_dropper_shifts to authenticated;

do $$
begin
  if exists (select 1 from pg_publication where pubname = 'supabase_realtime') then
    if not exists (
      select 1
      from pg_publication_tables
      where pubname = 'supabase_realtime'
        and schemaname = 'public'
        and tablename = 'dismissed_dropper_shifts'
    ) then
      execute 'alter publication supabase_realtime add table public.dismissed_dropper_shifts';
    end if;
  end if;
end;
$$;

create or replace function public.owner_delete_shift(
  p_shift_id text,
  p_dropper_user_id uuid default null,
  p_shift_start timestamptz default null,
  p_shift_end timestamptz default null
)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_key text := btrim(coalesce(p_shift_id, ''));
  v_shift_uuid uuid;
  v_user_id uuid := p_dropper_user_id;
  v_user_role text;
  v_start timestamptz := p_shift_start;
  v_end timestamptz := p_shift_end;
begin
  if not (select private.is_owner()) then
    raise exception 'Solo el owner puede eliminar turnos' using errcode = '42501';
  end if;
  if v_key = '' then
    raise exception 'Falta el turno a eliminar' using errcode = '22023';
  end if;

  if v_key ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' then
    v_shift_uuid := v_key::uuid;

    select s.user_id, p.role, coalesce(v_start, s.clock_in), coalesce(v_end, s.clock_out, now())
    into v_user_id, v_user_role, v_start, v_end
    from public.shifts as s
    left join public.profiles as p on p.id = s.user_id
    where s.id = v_shift_uuid;

    if v_user_id is null then
      v_user_id := p_dropper_user_id;
    end if;

    delete from public.dropper_shift_reports
    where shift_id = v_shift_uuid;

    delete from public.shifts
    where id = v_shift_uuid;
  end if;

  if v_user_id is not null and (p_dropper_user_id is not null or v_user_role = 'dropper') then
    insert into public.dismissed_dropper_shifts (
      dropper_user_id,
      shift_key,
      window_start,
      window_end
    )
    values (
      v_user_id,
      v_key,
      v_start,
      coalesce(v_end, now())
    )
    on conflict (dropper_user_id, shift_key) do update set
      window_start = coalesce(excluded.window_start, public.dismissed_dropper_shifts.window_start),
      window_end = coalesce(excluded.window_end, public.dismissed_dropper_shifts.window_end);
  end if;
end;
$$;

revoke all on function public.owner_delete_shift(text, uuid, timestamptz, timestamptz) from public, anon;
grant execute on function public.owner_delete_shift(text, uuid, timestamptz, timestamptz) to authenticated;

-- ============================================================================
-- 3. Muted / silenced OnlyFans fans (suppresses notifications across chatters)
-- ============================================================================

alter table public.fans
  add column if not exists is_muted boolean not null default false;

create index if not exists fans_model_muted_idx
  on public.fans (model_id, of_fan_id)
  where is_muted = true;

create or replace function public.set_fan_muted(
  p_model_id uuid,
  p_of_fan_id text,
  p_muted boolean
)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_fan_id text := regexp_replace(btrim(coalesce(p_of_fan_id, '')), '^[uU]', '');
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if v_fan_id = '' then
    return;
  end if;

  insert into public.fans (model_id, of_fan_id, is_muted, updated_at)
  values (p_model_id, v_fan_id, coalesce(p_muted, false), now())
  on conflict (model_id, of_fan_id) do update set
    is_muted = excluded.is_muted,
    updated_at = now();
end;
$$;

create or replace function public.upsert_fans_batch(
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

  -- Perform conflict-safe upsert preserving existing is_muted when item ->> 'is_muted' is null:
  with parsed as (
    select distinct on (btrim(item ->> 'of_fan_id'))
      btrim(item ->> 'of_fan_id') as of_fan_id,
      coalesce(btrim(item ->> 'username'), '') as username,
      nullif(btrim(item ->> 'sub_status'), '') as sub_status,
      (item ->> 'sub_cost')::numeric(12, 2) as sub_cost,
      nullif(btrim(item ->> 'sub_duration'), '') as sub_duration,
      (item ->> 'auto_renewal')::boolean as auto_renewal,
      (item ->> 'total_spend')::numeric(12, 2) as total_spend,
      (item ->> 'ppv_total')::numeric(12, 2) as ppv_total,
      (item ->> 'tip_total')::numeric(12, 2) as tip_total,
      coalesce((item ->> 'official_spend')::boolean, false) as official_spend,
      nullif(btrim(item ->> 'last_spend_at'), '')::timestamptz as last_spend_at,
      (item ->> 'highest_spend')::numeric(12, 2) as highest_spend,
      item -> 'lists_json' as lists_json,
      (item ? 'is_muted') and (item -> 'is_muted') <> 'null'::jsonb as has_muted,
      coalesce((item ->> 'is_muted')::boolean, false) as is_muted_val
    from jsonb_array_elements(p_items) as item
    where coalesce(btrim(item ->> 'of_fan_id'), '') <> ''
  ),
  upserted as (
    insert into public.fans (
      model_id,
      of_fan_id,
      username,
      sub_status,
      sub_cost,
      sub_duration,
      auto_renewal,
      total_spend,
      ppv_total,
      tip_total,
      official_spend,
      last_spend_at,
      highest_spend,
      lists_json,
      is_muted,
      updated_at
    )
    select
      p_model_id,
      parsed.of_fan_id,
      parsed.username,
      parsed.sub_status,
      parsed.sub_cost,
      parsed.sub_duration,
      parsed.auto_renewal,
      coalesce(parsed.total_spend, 0),
      coalesce(parsed.ppv_total, 0),
      coalesce(parsed.tip_total, 0),
      parsed.official_spend,
      parsed.last_spend_at,
      coalesce(parsed.highest_spend, 0),
      coalesce(parsed.lists_json, '[]'::jsonb),
      case when parsed.has_muted then parsed.is_muted_val else false end,
      now()
    from parsed
    on conflict (model_id, of_fan_id) do update set
      username = case when excluded.username <> '' then excluded.username else public.fans.username end,
      sub_status = coalesce(excluded.sub_status, public.fans.sub_status),
      sub_cost = coalesce(excluded.sub_cost, public.fans.sub_cost),
      sub_duration = coalesce(excluded.sub_duration, public.fans.sub_duration),
      auto_renewal = coalesce(excluded.auto_renewal, public.fans.auto_renewal),
      total_spend = case
        when excluded.official_spend then greatest(excluded.total_spend, public.fans.total_spend)
        when not public.fans.official_spend and excluded.total_spend > 0 then excluded.total_spend
        else public.fans.total_spend
      end,
      ppv_total = case when excluded.ppv_total > 0 then excluded.ppv_total else public.fans.ppv_total end,
      tip_total = case when excluded.tip_total > 0 then excluded.tip_total else public.fans.tip_total end,
      official_spend = public.fans.official_spend or excluded.official_spend,
      last_spend_at = coalesce(excluded.last_spend_at, public.fans.last_spend_at),
      highest_spend = greatest(excluded.highest_spend, public.fans.highest_spend),
      lists_json = case
        when excluded.lists_json is not null and excluded.lists_json <> '[]'::jsonb then excluded.lists_json
        else public.fans.lists_json
      end,
      updated_at = now()
    returning of_fan_id
  ),
  muted_updates as (
    update public.fans as f
    set is_muted = parsed.is_muted_val
    from parsed
    where f.model_id = p_model_id
      and f.of_fan_id = parsed.of_fan_id
      and parsed.has_muted
      and f.is_muted is distinct from parsed.is_muted_val
    returning f.of_fan_id
  )
  select count(*)::integer into v_rows from upserted;

  return v_rows;
end;
$$;

revoke all on function public.set_fan_muted(uuid, text, boolean) from public, anon;
revoke all on function public.upsert_fans_batch(uuid, jsonb) from public, anon;
grant execute on function public.set_fan_muted(uuid, text, boolean) to authenticated;
grant execute on function public.upsert_fans_batch(uuid, jsonb) to authenticated;
