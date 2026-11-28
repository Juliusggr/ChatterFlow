-- Fix Dashboard & Creator Reports daily revenue accuracy (00:00 to 23:59 local time):
-- Previously, public.model_earnings preferred synthetic 'chart:%' rows over real '/payouts/transactions' rows.
-- Because OnlyFans '/earnings/chart' groups days in UTC (20:00 prev day to 19:59 current day in America/Caracas)
-- and stamps them at 12:00 noon, any 'chart:%' row masked real transactions and dropped sales made after 20:00
-- local time (or after the last manual chart sync).
--
-- Now real transactions (of_transaction_id NOT LIKE 'chart:%') ALWAYS take priority, and 'chart:%' rows are only
-- used as a fallback on historical days that have zero real transactions for that (model_id, type, Caracas-day).

create index if not exists model_transactions_real_created_covering_idx
  on public.model_transactions (created_at desc)
  include (model_id, type, gross_amount, net_amount, of_fan_id)
  where of_transaction_id not like 'chart:%';

create index if not exists model_transactions_real_caracas_day_idx
  on public.model_transactions (model_id, type, ((created_at at time zone 'America/Caracas')::date))
  where of_transaction_id not like 'chart:%';

create or replace view public.model_earnings
with (security_invoker = true)
as
select t.*
from public.model_transactions as t
where t.created_at <> '1970-01-01T00:00:00Z'::timestamptz
  and (
    t.of_transaction_id not like 'chart:%'
    or not exists (
      select 1
      from public.model_transactions as r
      where r.model_id = t.model_id
        and r.of_transaction_id not like 'chart:%'
        and r.type = t.type
        and (r.created_at at time zone 'America/Caracas')::date
          = (t.created_at at time zone 'America/Caracas')::date
    )
  );

do $$
begin
  if exists (select 1 from pg_publication where pubname = 'supabase_realtime') then
    if not exists (
      select 1
      from pg_publication_tables
      where pubname = 'supabase_realtime'
        and schemaname = 'public'
        and tablename = 'sales'
    ) then
      execute 'alter publication supabase_realtime add table public.sales';
    end if;
    if not exists (
      select 1
      from pg_publication_tables
      where pubname = 'supabase_realtime'
        and schemaname = 'public'
        and tablename = 'model_transactions'
    ) then
      execute 'alter publication supabase_realtime add table public.model_transactions';
    end if;
  end if;
end;
$$;
