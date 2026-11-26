-- Complete migration of CreatorFloww local data to Supabase (zero SQLite).
-- Safe to run on top of existing CRM migrations (idempotent).

-- 1. Extend public.models with OnlyFans account & balance telemetry
alter table public.models add column if not exists of_user_id text;
alter table public.models add column if not exists partition_id text;
alter table public.models add column if not exists current_balance numeric(12, 2) not null default 0;
alter table public.models add column if not exists pending_balance numeric(12, 2) not null default 0;
alter table public.models add column if not exists last_synced_at timestamptz;
alter table public.models add column if not exists fans_count integer;

update public.models
set
  partition_id = coalesce(
    nullif(partition_id, ''),
    nullif(session_data ->> 'partition_id', ''),
    nullif(regexp_replace(external_key, '^partition:', ''), ''),
    id::text
  ),
  of_user_id = coalesce(
    nullif(of_user_id, ''),
    nullif(session_data ->> 'of_user_id', '')
  )
where partition_id is null or partition_id = '' or of_user_id is null;

alter table public.models alter column partition_id set not null;

create unique index if not exists models_of_user_id_unique_idx
  on public.models (of_user_id)
  where of_user_id is not null and of_user_id <> '';

create unique index if not exists models_partition_id_unique_idx
  on public.models (partition_id);

drop policy if exists models_update_managers on public.models;
drop policy if exists models_update_accessible on public.models;
create policy models_update_accessible on public.models
  for update to authenticated
  using ((select private.can_access_model(id)))
  with check ((select private.can_access_model(id)));

grant select (
  id, name, handle, avatar_url, external_key,
  of_user_id, partition_id, current_balance, pending_balance,
  last_synced_at, fans_count, created_at
) on public.models to authenticated;
grant insert (
  name, handle, avatar_url, session_data, external_key,
  of_user_id, partition_id, current_balance, pending_balance,
  last_synced_at, fans_count
) on public.models to authenticated;
grant update (
  name, handle, avatar_url, session_data, external_key,
  of_user_id, partition_id, current_balance, pending_balance,
  last_synced_at, fans_count
) on public.models to authenticated;

-- Reassign a fresh login partition onto an existing model if the same OF account logs in again.
create or replace function public.adopt_model_partition(
  p_placeholder_id uuid,
  p_of_user_id text,
  p_username text,
  p_display_name text,
  p_avatar_url text
)
returns jsonb
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_existing public.models;
  v_placeholder public.models;
  v_old_partition text;
  v_updated public.models;
begin
  if not (select private.is_manager()) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  if p_of_user_id is null or btrim(p_of_user_id) = '' then
    return null;
  end if;
  select * into v_existing
  from public.models
  where of_user_id = btrim(p_of_user_id)
    and id <> p_placeholder_id
  limit 1;
  if not found then
    return null;
  end if;
  select * into v_placeholder
  from public.models
  where id = p_placeholder_id
    and (of_user_id is null or of_user_id = '');
  if not found then
    return null;
  end if;
  v_old_partition := v_existing.partition_id;
  delete from public.models where id = p_placeholder_id;
  update public.models
  set
    partition_id = v_placeholder.partition_id,
    external_key = 'partition:' || v_placeholder.partition_id,
    handle = case when coalesce(btrim(p_username), '') <> '' then btrim(p_username) else handle end,
    name = case when coalesce(btrim(p_display_name), '') <> '' then btrim(p_display_name) else name end,
    avatar_url = coalesce(nullif(btrim(p_avatar_url), ''), avatar_url),
    last_synced_at = now()
  where id = v_existing.id
  returning * into v_updated;
  return jsonb_build_object(
    'model', to_jsonb(v_updated),
    'old_partition', v_old_partition
  );
end;
$$;

revoke all on function public.adopt_model_partition(uuid, text, text, text, text) from public, anon;
grant execute on function public.adopt_model_partition(uuid, text, text, text, text) to authenticated;

-- 2. Fans synced from OnlyFans
create table if not exists public.fans (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  of_fan_id text not null,
  username text not null default '',
  nickname text not null default '',
  total_spend numeric(12, 2) not null default 0,
  last_spend_at timestamptz,
  highest_spend numeric(12, 2) not null default 0,
  ppv_total numeric(12, 2) not null default 0,
  tip_total numeric(12, 2) not null default 0,
  sub_status text,
  sub_cost numeric(12, 2),
  sub_duration text,
  auto_renewal boolean,
  official_spend boolean not null default false,
  lists_json jsonb not null default '[]'::jsonb,
  updated_at timestamptz not null default now(),
  constraint fans_model_fan_unique unique (model_id, of_fan_id)
);

create index if not exists fans_model_id_idx on public.fans (model_id);

-- 3. Creator earnings transactions & deduplicated model_earnings view
create table if not exists public.model_transactions (
  id uuid primary key default gen_random_uuid(),
  of_transaction_id text not null unique,
  model_id uuid not null references public.models (id) on delete cascade,
  of_fan_id text,
  fan_username text not null default '',
  type text not null check (type in ('sub', 'tip', 'post', 'message', 'referral', 'stream')),
  gross_amount numeric(12, 2) not null default 0,
  net_amount numeric(12, 2) not null default 0,
  created_at timestamptz not null
);

create index if not exists model_transactions_model_created_idx
  on public.model_transactions (model_id, created_at desc);
create index if not exists model_transactions_model_type_created_idx
  on public.model_transactions (model_id, type, created_at desc);

create or replace view public.model_earnings
with (security_invoker = true)
as
select t.*
from public.model_transactions as t
where t.created_at <> '1970-01-01T00:00:00Z'::timestamptz
  and (
    t.of_transaction_id like ('chart:' || t.model_id::text || ':%')
    or not exists (
      select 1
      from public.model_transactions as c
      where c.model_id = t.model_id
        and c.of_transaction_id like ('chart:' || t.model_id::text || ':%')
        and c.type = t.type
        and (c.created_at at time zone 'UTC')::date = (t.created_at at time zone 'UTC')::date
    )
  );

-- 4. PPV timeline & chat tips (for live PPV unlock / tip notifications)
create table if not exists public.ppv_timeline (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  of_fan_id text not null,
  of_message_id text not null,
  price numeric(12, 2) not null,
  purchased boolean not null default false,
  created_at timestamptz not null,
  constraint ppv_timeline_model_msg_unique unique (model_id, of_message_id)
);

create index if not exists ppv_timeline_model_fan_idx
  on public.ppv_timeline (model_id, of_fan_id, created_at desc);

create table if not exists public.chat_tips (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  of_fan_id text not null,
  of_message_id text not null,
  amount numeric(12, 2) not null,
  created_at timestamptz not null,
  constraint chat_tips_model_msg_unique unique (model_id, of_message_id)
);

create index if not exists chat_tips_model_fan_idx
  on public.chat_tips (model_id, of_fan_id, created_at desc);

-- 5. Publications & fan arrivals (Dropper activity tracking)
create table if not exists public.publications (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  of_post_id text not null,
  created_at timestamptz not null,
  author_user_id uuid references public.profiles (id) on delete set null,
  constraint publications_model_post_unique unique (model_id, of_post_id)
);

create index if not exists publications_created_idx on public.publications (created_at desc);
create index if not exists publications_model_created_idx on public.publications (model_id, created_at desc);

create table if not exists public.fan_arrivals (
  id uuid primary key default gen_random_uuid(),
  model_id uuid not null references public.models (id) on delete cascade,
  of_fan_id text not null,
  arrived_at timestamptz not null,
  constraint fan_arrivals_model_fan_unique unique (model_id, of_fan_id)
);

create index if not exists fan_arrivals_arrived_idx on public.fan_arrivals (arrived_at desc);
create index if not exists fan_arrivals_model_arrived_idx on public.fan_arrivals (model_id, arrived_at desc);

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
  v_author uuid;
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  v_author := p_author_user_id;
  if v_author is null then
    select id into v_author
    from public.profiles
    where id = (select auth.uid())
      and role = 'dropper'
      and is_active;
  end if;
  insert into public.publications (model_id, of_post_id, created_at, author_user_id)
  values (p_model_id, btrim(p_of_post_id), coalesce(p_created_at, now()), v_author)
  on conflict (model_id, of_post_id) do update
    set author_user_id = coalesce(public.publications.author_user_id, excluded.author_user_id)
    where public.publications.author_user_id is null and excluded.author_user_id is not null;
  get diagnostics v_rows = row_count;
  return v_rows > 0;
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
  v_rows integer := 0;
begin
  if not (select private.can_access_model(p_model_id)) then
    raise exception 'not allowed' using errcode = '42501';
  end if;
  insert into public.fan_arrivals (model_id, of_fan_id, arrived_at)
  values (p_model_id, btrim(p_of_fan_id), coalesce(p_arrived_at, now()))
  on conflict (model_id, of_fan_id) do nothing;
  get diagnostics v_rows = row_count;
  return v_rows > 0;
end;
$$;

revoke all on function public.record_publication(uuid, text, timestamptz, uuid) from public, anon;
revoke all on function public.record_fan_arrival(uuid, text, timestamptz) from public, anon;
grant execute on function public.record_publication(uuid, text, timestamptz, uuid) to authenticated;
grant execute on function public.record_fan_arrival(uuid, text, timestamptz) to authenticated;

-- 6. Dropper shift reports (one persistent record per shift & model)
create table if not exists public.dropper_shift_reports (
  id uuid primary key default gen_random_uuid(),
  shift_id uuid not null references public.shifts (id) on delete cascade,
  dropper_user_id uuid not null references public.profiles (id) on delete cascade,
  model_id uuid not null references public.models (id) on delete cascade,
  shift_start timestamptz not null,
  shift_end timestamptz,
  schedule_label text not null default '',
  status text not null default 'active',
  publications_count integer not null default 0,
  subscriptions_count integer not null default 0,
  updated_at timestamptz not null default now(),
  constraint dropper_shift_reports_status_check check (status in ('active', 'completed')),
  constraint dropper_shift_reports_counts_check check (publications_count >= 0 and subscriptions_count >= 0),
  constraint dropper_shift_reports_shift_model_unique unique (shift_id, model_id)
);

create index if not exists dropper_shift_reports_start_idx
  on public.dropper_shift_reports (shift_start desc);
create index if not exists dropper_shift_reports_user_start_idx
  on public.dropper_shift_reports (dropper_user_id, shift_start desc);

-- 7. Notification state, deduplication, and history
create table if not exists public.account_notify_state (
  model_id uuid primary key references public.models (id) on delete cascade,
  last_seen_message_id text not null default '',
  last_message_timestamp timestamptz,
  unread_count integer not null default 0,
  inbox_seeded boolean not null default false,
  transactions_seeded boolean not null default false
);

create table if not exists public.notified_events (
  model_id uuid not null references public.models (id) on delete cascade,
  event_key text not null,
  notified_at timestamptz not null default now(),
  primary key (model_id, event_key)
);

create index if not exists notified_events_at_idx on public.notified_events (notified_at);

create table if not exists public.notification_history (
  id text primary key,
  model_id uuid not null references public.models (id) on delete cascade,
  kind text not null,
  model_name text not null default '',
  model_username text not null default '',
  model_avatar text,
  of_fan_id text not null default '',
  fan_name text not null default '',
  amount numeric(12, 2),
  net_amount numeric(12, 2),
  preview text not null default '',
  created_at timestamptz not null default now()
);

create index if not exists notification_history_created_idx
  on public.notification_history (created_at desc);

-- 8. Row Level Security on all new tables
alter table public.fans enable row level security;
alter table public.model_transactions enable row level security;
alter table public.ppv_timeline enable row level security;
alter table public.chat_tips enable row level security;
alter table public.publications enable row level security;
alter table public.fan_arrivals enable row level security;
alter table public.dropper_shift_reports enable row level security;
alter table public.account_notify_state enable row level security;
alter table public.notified_events enable row level security;
alter table public.notification_history enable row level security;

alter table public.fans force row level security;
alter table public.model_transactions force row level security;
alter table public.ppv_timeline force row level security;
alter table public.chat_tips force row level security;
alter table public.publications force row level security;
alter table public.fan_arrivals force row level security;
alter table public.dropper_shift_reports force row level security;
alter table public.account_notify_state force row level security;
alter table public.notified_events force row level security;
alter table public.notification_history force row level security;

-- Helper macro pattern: model-scoped policies
drop policy if exists fans_access on public.fans;
create policy fans_access on public.fans
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists model_transactions_access on public.model_transactions;
create policy model_transactions_access on public.model_transactions
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists ppv_timeline_access on public.ppv_timeline;
create policy ppv_timeline_access on public.ppv_timeline
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists chat_tips_access on public.chat_tips;
create policy chat_tips_access on public.chat_tips
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists publications_access on public.publications;
create policy publications_access on public.publications
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists fan_arrivals_access on public.fan_arrivals;
create policy fan_arrivals_access on public.fan_arrivals
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists dropper_shift_reports_select on public.dropper_shift_reports;
create policy dropper_shift_reports_select on public.dropper_shift_reports
  for select to authenticated
  using (
    (select private.is_manager())
    or dropper_user_id = (select auth.uid())
  );

drop policy if exists dropper_shift_reports_insert on public.dropper_shift_reports;
create policy dropper_shift_reports_insert on public.dropper_shift_reports
  for insert to authenticated
  with check (
    (select private.is_manager())
    or (
      dropper_user_id = (select auth.uid())
      and (select private.can_access_model(model_id))
    )
  );

drop policy if exists dropper_shift_reports_update on public.dropper_shift_reports;
create policy dropper_shift_reports_update on public.dropper_shift_reports
  for update to authenticated
  using (
    (select private.is_manager())
    or dropper_user_id = (select auth.uid())
  )
  with check (
    (select private.is_manager())
    or (
      dropper_user_id = (select auth.uid())
      and (select private.can_access_model(model_id))
    )
  );

drop policy if exists account_notify_state_access on public.account_notify_state;
create policy account_notify_state_access on public.account_notify_state
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists notified_events_access on public.notified_events;
create policy notified_events_access on public.notified_events
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

drop policy if exists notification_history_access on public.notification_history;
create policy notification_history_access on public.notification_history
  for all to authenticated
  using ((select private.can_access_model(model_id)))
  with check ((select private.can_access_model(model_id)));

-- Grants
revoke all on table public.fans from public, anon;
revoke all on table public.model_transactions from public, anon;
revoke all on table public.model_earnings from public, anon;
revoke all on table public.ppv_timeline from public, anon;
revoke all on table public.chat_tips from public, anon;
revoke all on table public.publications from public, anon;
revoke all on table public.fan_arrivals from public, anon;
revoke all on table public.dropper_shift_reports from public, anon;
revoke all on table public.account_notify_state from public, anon;
revoke all on table public.notified_events from public, anon;
revoke all on table public.notification_history from public, anon;

grant select, insert, update, delete on public.fans to authenticated;
grant select, insert, update, delete on public.model_transactions to authenticated;
grant select on public.model_earnings to authenticated;
grant select, insert, update, delete on public.ppv_timeline to authenticated;
grant select, insert, update, delete on public.chat_tips to authenticated;
grant select, insert, update, delete on public.publications to authenticated;
grant select, insert, update, delete on public.fan_arrivals to authenticated;
grant select, insert, update on public.dropper_shift_reports to authenticated;
grant select, insert, update, delete on public.account_notify_state to authenticated;
grant select, insert, update, delete on public.notified_events to authenticated;
grant select, insert, update, delete on public.notification_history to authenticated;

grant all on public.fans to service_role;
grant all on public.model_transactions to service_role;
grant all on public.model_earnings to service_role;
grant all on public.ppv_timeline to service_role;
grant all on public.chat_tips to service_role;
grant all on public.publications to service_role;
grant all on public.fan_arrivals to service_role;
grant all on public.dropper_shift_reports to service_role;
grant all on public.account_notify_state to service_role;
grant all on public.notified_events to service_role;
grant all on public.notification_history to service_role;

-- Enable Supabase Realtime on live tables
do $$
declare
  tbl text;
begin
  foreach tbl in array array[
    'models',
    'publications',
    'fan_arrivals',
    'dropper_shift_reports',
    'notification_history',
    'account_notify_state'
  ]
  loop
    if not exists (
      select 1
      from pg_publication_tables
      where pubname = 'supabase_realtime'
        and schemaname = 'public'
        and tablename = tbl
    ) then
      execute format('alter publication supabase_realtime add table public.%I', tbl);
    end if;
  end loop;
end $$;
