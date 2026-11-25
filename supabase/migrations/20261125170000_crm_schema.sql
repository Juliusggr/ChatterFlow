-- CreatorFloww agency schema.
-- Helpers live in private so PostgREST cannot call them.
-- RLS policies use (select fn()) so Postgres caches the check per statement.

create schema if not exists private;

revoke all on schema private from public, anon;
grant usage on schema private to authenticated, service_role;

create table public.profiles (
  id uuid primary key references auth.users (id) on delete cascade,
  username text not null,
  full_name text not null default '',
  role text not null,
  is_active boolean not null default true,
  created_at timestamptz not null default now(),
  constraint profiles_username_format check (username ~ '^[a-z0-9_]{3,32}$'),
  constraint profiles_role_check check (role in ('owner', 'admin', 'chatter', 'dropper'))
);

create unique index profiles_username_lower_idx on public.profiles (lower(username));
create index profiles_role_active_idx on public.profiles (role) where is_active;

create table public.models (
  id uuid primary key default gen_random_uuid(),
  name text not null,
  handle text not null default '',
  avatar_url text,
  session_data jsonb not null default '{}'::jsonb,
  external_key text not null,
  created_at timestamptz not null default now(),
  constraint models_external_key_unique unique (external_key)
);

create table public.model_assignments (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles (id) on delete cascade,
  model_id uuid not null references public.models (id) on delete cascade,
  assigned_by uuid references public.profiles (id) on delete set null,
  created_at timestamptz not null default now(),
  constraint model_assignments_user_model_unique unique (user_id, model_id)
);

create index model_assignments_model_id_idx on public.model_assignments (model_id);
create index model_assignments_assigned_by_idx on public.model_assignments (assigned_by);

create table public.schedules (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles (id) on delete cascade,
  day_of_week smallint not null,
  start_time time not null,
  end_time time not null,
  constraint schedules_day_check check (day_of_week between 0 and 6),
  constraint schedules_time_check check (start_time < end_time),
  constraint schedules_user_day_start_unique unique (user_id, day_of_week, start_time)
);

create table public.shifts (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles (id) on delete cascade,
  clock_in timestamptz not null default now(),
  clock_out timestamptz,
  status text not null default 'active',
  total_minutes integer,
  constraint shifts_status_check check (status in ('active', 'completed')),
  constraint shifts_completed_check check (
    (status = 'active' and clock_out is null and total_minutes is null)
    or (status = 'completed' and clock_out is not null and total_minutes is not null and clock_out >= clock_in)
  )
);

create unique index shifts_one_active_per_user_idx
  on public.shifts (user_id)
  where status = 'active';
create index shifts_user_clock_in_idx on public.shifts (user_id, clock_in desc);

create table public.fan_notes (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  fan_platform_id text not null,
  fan_username text not null default '',
  notes text not null default '',
  updated_by uuid references public.profiles (id) on delete set null,
  updated_at timestamptz not null default now(),
  constraint fan_notes_model_fan_unique unique (model_id, fan_platform_id)
);

create index fan_notes_updated_by_idx on public.fan_notes (updated_by);
alter table public.fan_notes replica identity full;

create table public.sales (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  employee_id uuid not null references public.profiles (id) on delete restrict,
  fan_platform_id text not null default '',
  amount_gross numeric(12, 2) not null,
  amount_net numeric(12, 2) not null,
  category text not null,
  recorded_at timestamptz not null default now(),
  source_id text,
  constraint sales_amounts_check check (amount_gross >= 0 and amount_net >= 0),
  constraint sales_category_check check (
    category in ('subscription', 'post', 'message', 'tip', 'referral', 'stream', 'ppv')
  ),
  constraint sales_source_id_unique unique (source_id)
);

create index sales_employee_recorded_idx on public.sales (employee_id, recorded_at desc);
create index sales_model_recorded_idx on public.sales (model_id, recorded_at desc);
create index sales_fan_idx on public.sales (model_id, fan_platform_id);

create or replace function private.is_active_member()
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
  );
$$;

create or replace function private.is_manager()
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
      and role in ('owner', 'admin')
  );
$$;

create or replace function private.can_access_model(target_model uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select (select private.is_manager())
    or exists (
      select 1
      from public.model_assignments assignment
      join public.profiles profile on profile.id = assignment.user_id
      where assignment.model_id = target_model
        and assignment.user_id = (select auth.uid())
        and profile.is_active
        and profile.role in ('chatter', 'dropper')
    );
$$;

create or replace function private.has_active_shift(target_user uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
  select exists (
    select 1
    from public.shifts
    where user_id = target_user
      and status = 'active'
  );
$$;

revoke all on function private.is_active_member() from public, anon;
revoke all on function private.is_manager() from public, anon;
revoke all on function private.can_access_model(uuid) from public, anon;
revoke all on function private.has_active_shift(uuid) from public, anon;
grant execute on function private.is_active_member() to authenticated, service_role;
grant execute on function private.is_manager() to authenticated, service_role;
grant execute on function private.can_access_model(uuid) to authenticated, service_role;
grant execute on function private.has_active_shift(uuid) to authenticated, service_role;

alter table public.profiles enable row level security;
alter table public.models enable row level security;
alter table public.model_assignments enable row level security;
alter table public.schedules enable row level security;
alter table public.shifts enable row level security;
alter table public.fan_notes enable row level security;
alter table public.sales enable row level security;

alter table public.profiles force row level security;
alter table public.models force row level security;
alter table public.model_assignments force row level security;
alter table public.schedules force row level security;
alter table public.shifts force row level security;
alter table public.fan_notes force row level security;
alter table public.sales force row level security;

create policy profiles_select_members on public.profiles
  for select to authenticated
  using ((select private.is_active_member()));

create policy profiles_insert_managers on public.profiles
  for insert to authenticated
  with check ((select private.is_manager()));

create policy profiles_update_managers on public.profiles
  for update to authenticated
  using ((select private.is_manager()))
  with check ((select private.is_manager()));

create policy profiles_delete_managers on public.profiles
  for delete to authenticated
  using ((select private.is_manager()));

create policy models_select_assigned on public.models
  for select to authenticated
  using ((select private.can_access_model(id)));

create policy models_insert_managers on public.models
  for insert to authenticated
  with check ((select private.is_manager()));

create policy models_update_managers on public.models
  for update to authenticated
  using ((select private.is_manager()))
  with check ((select private.is_manager()));

create policy models_delete_managers on public.models
  for delete to authenticated
  using ((select private.is_manager()));

create policy assignments_select on public.model_assignments
  for select to authenticated
  using (
    (select private.is_manager())
    or user_id = (select auth.uid())
  );

create policy assignments_write_managers on public.model_assignments
  for insert to authenticated
  with check ((select private.is_manager()));

create policy assignments_update_managers on public.model_assignments
  for update to authenticated
  using ((select private.is_manager()))
  with check ((select private.is_manager()));

create policy assignments_delete_managers on public.model_assignments
  for delete to authenticated
  using ((select private.is_manager()));

create policy schedules_select on public.schedules
  for select to authenticated
  using (
    (select private.is_manager())
    or user_id = (select auth.uid())
  );

create policy schedules_write_managers on public.schedules
  for insert to authenticated
  with check ((select private.is_manager()));

create policy schedules_update_managers on public.schedules
  for update to authenticated
  using ((select private.is_manager()))
  with check ((select private.is_manager()));

create policy schedules_delete_managers on public.schedules
  for delete to authenticated
  using ((select private.is_manager()));

create policy shifts_select_members on public.shifts
  for select to authenticated
  using ((select private.is_active_member()));

create policy shifts_insert_self on public.shifts
  for insert to authenticated
  with check (
    user_id = (select auth.uid())
    and status = 'active'
    and (select private.is_active_member())
    and not (select private.has_active_shift((select auth.uid())))
  );

create policy shifts_clock_out_self on public.shifts
  for update to authenticated
  using (user_id = (select auth.uid()) and status = 'active')
  with check (
    user_id = (select auth.uid())
    and status = 'completed'
    and clock_out is not null
  );

create policy fan_notes_select on public.fan_notes
  for select to authenticated
  using ((select private.can_access_model(model_id)));

create policy fan_notes_insert on public.fan_notes
  for insert to authenticated
  with check (
    (select private.can_access_model(model_id))
    and updated_by = (select auth.uid())
  );

create policy fan_notes_update on public.fan_notes
  for update to authenticated
  using ((select private.can_access_model(model_id)))
  with check (
    (select private.can_access_model(model_id))
    and updated_by = (select auth.uid())
  );

create policy fan_notes_delete_managers on public.fan_notes
  for delete to authenticated
  using ((select private.is_manager()));

create policy sales_select on public.sales
  for select to authenticated
  using (
    employee_id = (select auth.uid())
    or (select private.can_access_model(model_id))
  );

revoke all on table public.profiles from public, anon;
revoke all on table public.models from public, anon;
revoke all on table public.model_assignments from public, anon;
revoke all on table public.schedules from public, anon;
revoke all on table public.shifts from public, anon;
revoke all on table public.fan_notes from public, anon;
revoke all on table public.sales from public, anon;

grant select, insert, update, delete on public.profiles to authenticated;
grant select (id, name, handle, avatar_url, external_key, created_at) on public.models to authenticated;
grant insert (name, handle, avatar_url, session_data, external_key) on public.models to authenticated;
grant update (name, handle, avatar_url, session_data) on public.models to authenticated;
grant delete on public.models to authenticated;
grant select, insert, update, delete on public.model_assignments to authenticated;
grant select, insert, update, delete on public.schedules to authenticated;
grant select, insert, update on public.shifts to authenticated;
grant select, insert, update, delete on public.fan_notes to authenticated;
grant select on public.sales to authenticated;

grant all on public.profiles to service_role;
grant all on public.models to service_role;
grant all on public.model_assignments to service_role;
grant all on public.schedules to service_role;
grant all on public.shifts to service_role;
grant all on public.fan_notes to service_role;
grant all on public.sales to service_role;

create or replace function public.clock_in()
returns public.shifts
language plpgsql
security definer
set search_path = ''
as $$
declare
  created public.shifts;
begin
  if not (select private.is_active_member()) then
    raise exception 'Cuenta inactiva' using errcode = '42501';
  end if;
  if (select private.has_active_shift((select auth.uid()))) then
    raise exception 'Ya tienes un turno activo' using errcode = '23505';
  end if;
  insert into public.shifts (user_id, clock_in, status)
  values ((select auth.uid()), now(), 'active')
  returning * into created;
  return created;
end;
$$;

create or replace function public.clock_out()
returns public.shifts
language plpgsql
security definer
set search_path = ''
as $$
declare
  closed public.shifts;
begin
  update public.shifts
  set
    clock_out = now(),
    status = 'completed',
    total_minutes = greatest(0, floor(extract(epoch from (now() - clock_in)) / 60)::integer)
  where user_id = (select auth.uid())
    and status = 'active'
  returning * into closed;
  if not found then
    raise exception 'No tienes un turno activo' using errcode = 'P0002';
  end if;
  return closed;
end;
$$;

create or replace function public.model_session(p_model_id uuid)
returns jsonb
language plpgsql
stable
security definer
set search_path = ''
as $$
begin
  if not (select private.is_manager()) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  return (
    select session_data
    from public.models
    where id = p_model_id
  );
end;
$$;

create or replace function public.record_sale(
  p_external_key text,
  p_fan_platform_id text,
  p_amount_gross numeric,
  p_amount_net numeric,
  p_category text,
  p_recorded_at timestamptz,
  p_source_id text
)
returns uuid
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_model uuid;
  v_user uuid := (select auth.uid());
  v_id uuid;
begin
  if v_user is null or not (select private.is_active_member()) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if not (select private.has_active_shift(v_user)) then
    raise exception 'Se necesita un turno activo para registrar la venta' using errcode = '42501';
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
    coalesce(p_recorded_at, now()),
    p_source_id
  )
  on conflict (source_id) do nothing
  returning id into v_id;
  return v_id;
end;
$$;

revoke all on function public.clock_in() from public, anon;
revoke all on function public.clock_out() from public, anon;
revoke all on function public.model_session(uuid) from public, anon;
revoke all on function public.record_sale(text, text, numeric, numeric, text, timestamptz, text) from public, anon;
grant execute on function public.clock_in() to authenticated;
grant execute on function public.clock_out() to authenticated;
grant execute on function public.model_session(uuid) to authenticated;
grant execute on function public.record_sale(text, text, numeric, numeric, text, timestamptz, text) to authenticated;

create or replace function private.guard_last_owner()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  if old.role = 'owner'
    and (new.role <> 'owner' or new.is_active = false)
    and not exists (
      select 1
      from public.profiles
      where role = 'owner'
        and is_active
        and id <> old.id
    )
  then
    raise exception 'Debe quedar al menos un owner activo';
  end if;
  return new;
end;
$$;

create trigger profiles_guard_last_owner
  before update on public.profiles
  for each row
  execute function private.guard_last_owner();

create or replace function private.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_username text := lower(coalesce(new.raw_app_meta_data ->> 'username', split_part(new.email, '@', 1)));
  v_role text := coalesce(new.raw_app_meta_data ->> 'role', 'chatter');
begin
  if v_role not in ('owner', 'admin', 'chatter', 'dropper') then
    v_role := 'chatter';
  end if;
  insert into public.profiles (id, username, full_name, role)
  values (
    new.id,
    v_username,
    coalesce(new.raw_app_meta_data ->> 'full_name', v_username),
    v_role
  )
  on conflict (id) do nothing;
  return new;
end;
$$;

create trigger on_auth_user_created
  after insert on auth.users
  for each row
  execute function private.handle_new_user();

do $$
begin
  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'supabase_realtime'
      and schemaname = 'public'
      and tablename = 'fan_notes'
  ) then
    alter publication supabase_realtime add table public.fan_notes;
  end if;
  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'supabase_realtime'
      and schemaname = 'public'
      and tablename = 'shifts'
  ) then
    alter publication supabase_realtime add table public.shifts;
  end if;
end $$;
