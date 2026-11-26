-- Performance optimization migration for CreatorFloww on Supabase:
-- 1. Replaces per-row RLS function calls with per-statement InitPlan array checks.
-- 2. Adds missing covering & partial indexes for time-range scans and chart deduplication.
-- 3. Optimizes public.model_earnings view to use an Index-Only Scan for chart lookups.
-- 4. Adds batch write RPCs and server-side report aggregation RPCs to eliminate N+1 round-trips.

-- ============================================================================
-- 1. Fast InitPlan helper for RLS policies
-- ============================================================================
create or replace function private.my_assigned_model_ids()
returns uuid[]
language sql
stable
security definer
set search_path = ''
as $$
  select coalesce(array_agg(assignment.model_id), '{}'::uuid[])
  from public.model_assignments assignment
  join public.profiles profile on profile.id = assignment.user_id
  where assignment.user_id = (select auth.uid())
    and profile.is_active
    and profile.role in ('chatter', 'dropper');
$$;

revoke all on function private.my_assigned_model_ids() from public, anon;
grant execute on function private.my_assigned_model_ids() to authenticated, service_role;

-- Upgrade model-scoped RLS policies so Postgres evaluates (select private.is_manager())
-- and (select private.my_assigned_model_ids()) ONCE per statement (InitPlan) instead of per row.
drop policy if exists models_select_assigned on public.models;
create policy models_select_assigned on public.models
  for select to authenticated
  using (
    (select private.is_manager())
    or id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists models_update_accessible on public.models;
create policy models_update_accessible on public.models
  for update to authenticated
  using (
    (select private.is_manager())
    or id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists fan_notes_select on public.fan_notes;
create policy fan_notes_select on public.fan_notes
  for select to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists fan_notes_insert on public.fan_notes;
create policy fan_notes_insert on public.fan_notes
  for insert to authenticated
  with check (
    (
      (select private.is_manager())
      or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
    )
    and updated_by = (select auth.uid())
  );

drop policy if exists fan_notes_update on public.fan_notes;
create policy fan_notes_update on public.fan_notes
  for update to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (
      (select private.is_manager())
      or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
    )
    and updated_by = (select auth.uid())
  );

drop policy if exists sales_select on public.sales;
create policy sales_select on public.sales
  for select to authenticated
  using (
    (select private.is_manager())
    or employee_id = (select auth.uid())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists fans_access on public.fans;
create policy fans_access on public.fans
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists model_transactions_access on public.model_transactions;
create policy model_transactions_access on public.model_transactions
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists ppv_timeline_access on public.ppv_timeline;
create policy ppv_timeline_access on public.ppv_timeline
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists chat_tips_access on public.chat_tips;
create policy chat_tips_access on public.chat_tips
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists publications_access on public.publications;
create policy publications_access on public.publications
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists fan_arrivals_access on public.fan_arrivals;
create policy fan_arrivals_access on public.fan_arrivals
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists account_notify_state_access on public.account_notify_state;
create policy account_notify_state_access on public.account_notify_state
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists notified_events_access on public.notified_events;
create policy notified_events_access on public.notified_events
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

drop policy if exists notification_history_access on public.notification_history;
create policy notification_history_access on public.notification_history
  for all to authenticated
  using (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  )
  with check (
    (select private.is_manager())
    or model_id = any (coalesce((select private.my_assigned_model_ids()), '{}'::uuid[]))
  );

-- ============================================================================
-- 2. Missing covering & partial indexes + faster model_earnings view
-- ============================================================================

-- Covering index for Dashboard & Employee Reports date-range scans on sales
create index if not exists sales_recorded_at_covering_idx
  on public.sales (recorded_at desc)
  include (employee_id, model_id, amount_gross, amount_net);

-- Leading timestamp index for cross-model range scans on model_transactions
create index if not exists model_transactions_created_covering_idx
  on public.model_transactions (created_at desc)
  include (model_id, type, gross_amount, net_amount, of_transaction_id);

-- Partial expression index for instant chart-vs-payout deduplication in model_earnings
create index if not exists model_transactions_chart_utc_day_idx
  on public.model_transactions (model_id, type, ((created_at at time zone 'UTC')::date))
  where of_transaction_id like 'chart:%';

-- Active shifts ordered by clock_in (Dashboard live panel)
create index if not exists shifts_active_clock_in_idx
  on public.shifts (clock_in asc)
  where status = 'active';

-- Unread badge lookup index
create index if not exists account_notify_state_unread_idx
  on public.account_notify_state (model_id)
  include (unread_count)
  where unread_count > 0;

-- Models sort order used by list_models()
create index if not exists models_name_handle_idx
  on public.models (name asc, handle asc, id asc);

-- Rewrite model_earnings to use constant prefix 'chart:%' so Postgres uses model_transactions_chart_utc_day_idx
create or replace view public.model_earnings
with (security_invoker = true)
as
select t.*
from public.model_transactions as t
where t.created_at <> '1970-01-01T00:00:00Z'::timestamptz
  and (
    t.of_transaction_id like 'chart:%'
    or not exists (
      select 1
      from public.model_transactions as c
      where c.model_id = t.model_id
        and c.of_transaction_id like 'chart:%'
        and c.type = t.type
        and (c.created_at at time zone 'UTC')::date = (t.created_at at time zone 'UTC')::date
    )
  );

-- ============================================================================
-- 3. Batch write RPCs (collapse N+1 HTTP calls during OnlyFans sync into 1)
-- ============================================================================

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
  on conflict (model_id, of_post_id) do update
    set author_user_id = coalesce(public.publications.author_user_id, excluded.author_user_id)
    where public.publications.author_user_id is null and excluded.author_user_id is not null;

  get diagnostics v_rows = row_count;
  return v_rows;
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
  on conflict (model_id, of_fan_id) do nothing;

  get diagnostics v_rows = row_count;
  return v_rows;
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
      item -> 'lists_json' as lists_json
    from jsonb_array_elements(p_items) as item
    where coalesce(btrim(item ->> 'of_fan_id'), '') <> ''
  )
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
    updated_at = now();

  get diagnostics v_rows = row_count;
  return v_rows;
end;
$$;

revoke all on function public.record_publications_batch(uuid, jsonb, uuid) from public, anon;
revoke all on function public.record_fan_arrivals_batch(uuid, jsonb) from public, anon;
revoke all on function public.upsert_fans_batch(uuid, jsonb) from public, anon;
grant execute on function public.record_publications_batch(uuid, jsonb, uuid) to authenticated;
grant execute on function public.record_fan_arrivals_batch(uuid, jsonb) to authenticated;
grant execute on function public.upsert_fans_batch(uuid, jsonb) to authenticated;

-- ============================================================================
-- 4. Server-side report aggregation RPCs (avoid transferring raw rows)
-- ============================================================================

create or replace function public.creator_report_agg(
  p_from timestamptz,
  p_to timestamptz,
  p_prev_from timestamptz,
  p_prev_to timestamptz,
  p_bucket text,
  p_tz text
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  v_tz text := 'UTC';
  v_min_from timestamptz;
  v_max_to timestamptz;
  v_result jsonb;
begin
  if p_tz is not null and exists (select 1 from pg_timezone_names where name = p_tz) then
    v_tz := p_tz;
  end if;

  if p_from is not null and p_prev_from is not null then
    v_min_from := least(p_from, p_prev_from);
  else
    v_min_from := p_from;
  end if;
  v_max_to := coalesce(greatest(p_to, p_prev_to), p_to);

  with scoped as (
    select
      e.model_id,
      e.type,
      e.gross_amount,
      e.net_amount,
      e.created_at,
      (p_from is null or e.created_at >= p_from) and e.created_at < p_to as in_cur,
      (p_prev_from is not null and p_prev_to is not null and e.created_at >= p_prev_from and e.created_at < p_prev_to) as in_prev
    from public.model_earnings as e
    where (v_min_from is null or e.created_at >= v_min_from)
      and e.created_at < v_max_to
  ),
  cur_totals as (
    select
      coalesce(sum(gross_amount), 0) as total_gross,
      coalesce(sum(net_amount), 0) as total_net,
      coalesce(sum(gross_amount) filter (where type = 'sub'), 0) as sub_gross,
      coalesce(sum(net_amount) filter (where type = 'sub'), 0) as sub_net,
      coalesce(sum(gross_amount) filter (where type = 'message'), 0) as msg_gross,
      coalesce(sum(net_amount) filter (where type = 'message'), 0) as msg_net,
      coalesce(sum(gross_amount) filter (where type = 'tip'), 0) as tip_gross,
      coalesce(sum(net_amount) filter (where type = 'tip'), 0) as tip_net
    from scoped
    where in_cur
  ),
  prev_totals as (
    select
      coalesce(sum(gross_amount), 0) as total_gross,
      coalesce(sum(net_amount), 0) as total_net,
      coalesce(sum(gross_amount) filter (where type = 'sub'), 0) as sub_gross,
      coalesce(sum(net_amount) filter (where type = 'sub'), 0) as sub_net,
      coalesce(sum(gross_amount) filter (where type = 'message'), 0) as msg_gross,
      coalesce(sum(net_amount) filter (where type = 'message'), 0) as msg_net,
      coalesce(sum(gross_amount) filter (where type = 'tip'), 0) as tip_gross,
      coalesce(sum(net_amount) filter (where type = 'tip'), 0) as tip_net
    from scoped
    where in_prev
  ),
  buckets as (
    select
      case
        when p_bucket = 'day' then to_char(created_at at time zone v_tz, 'YYYY-MM-DD')
        else to_char(created_at at time zone v_tz, 'YYYY-MM')
      end as bucket_key,
      coalesce(sum(gross_amount), 0) as gross,
      coalesce(sum(net_amount), 0) as net
    from scoped
    where in_cur
    group by 1
  ),
  per_model as (
    select
      model_id,
      coalesce(sum(gross_amount) filter (where type = 'message'), 0) as msg_gross,
      coalesce(sum(net_amount) filter (where type = 'message'), 0) as msg_net,
      coalesce(sum(gross_amount), 0) as total_gross,
      coalesce(sum(net_amount), 0) as total_net
    from scoped
    where in_cur
    group by model_id
  )
  select jsonb_build_object(
    'cur', (select to_jsonb(cur_totals) from cur_totals),
    'prev', case when p_prev_from is not null then (select to_jsonb(prev_totals) from prev_totals) else null end,
    'buckets', coalesce((select jsonb_agg(to_jsonb(buckets)) from buckets), '[]'::jsonb),
    'models', coalesce((select jsonb_agg(to_jsonb(per_model)) from per_model), '[]'::jsonb)
  )
  into v_result;

  return v_result;
end;
$$;

revoke all on function public.creator_report_agg(timestamptz, timestamptz, timestamptz, timestamptz, text, text) from public, anon;
grant execute on function public.creator_report_agg(timestamptz, timestamptz, timestamptz, timestamptz, text, text) to authenticated;
