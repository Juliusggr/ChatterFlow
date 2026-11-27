-- Reconcile last_spend_at / highest_spend from payout transactions plus chat
-- events that have not landed in model_transactions yet.
-- fans, ppv_timeline, chat_tips, publications and fan_arrivals already have
-- the composite unique keys used by ON CONFLICT. This migration does not
-- create them again.

create or replace function public.refresh_fan_spend_peaks(p_model_id uuid)
returns void
language plpgsql
volatile
security invoker
set search_path = ''
as $$
begin
  update public.fans as fan
  set last_spend_at = case
        when peaks.last_spend_at is null then fan.last_spend_at
        when fan.last_spend_at is null or peaks.last_spend_at > fan.last_spend_at then peaks.last_spend_at
        else fan.last_spend_at
      end,
      highest_spend = greatest(fan.highest_spend, coalesce(peaks.highest_spend, 0))
  from (
    select of_fan_id,
           max(spent_at) as last_spend_at,
           max(amount) as highest_spend
    from (
      select of_fan_id, created_at as spent_at, gross_amount as amount
      from public.model_transactions
      where model_id = p_model_id
        and of_fan_id is not null
        and btrim(of_fan_id) <> ''
        and of_transaction_id not like 'chart:%'
      union all
      select of_fan_id, created_at, price
      from public.ppv_timeline
      where model_id = p_model_id
        and purchased
        and of_fan_id is not null
        and btrim(of_fan_id) <> ''
      union all
      select of_fan_id, created_at, amount
      from public.chat_tips
      where model_id = p_model_id
        and of_fan_id is not null
        and btrim(of_fan_id) <> ''
    ) spend
    group by of_fan_id
  ) peaks
  where fan.model_id = p_model_id
    and fan.of_fan_id = peaks.of_fan_id;
end;
$$;

create or replace function public.fan_spend_breakdown(
  p_model_id uuid,
  p_of_fan_id text
)
returns table (
  ppv_spent numeric,
  tip_spent numeric,
  sub_spent numeric,
  other_spent numeric,
  total_spend_gross numeric,
  total_earned_net numeric,
  last_spend_at timestamptz,
  highest_spend numeric
)
language sql
stable
security invoker
set search_path = ''
as $$
  with fan_tx as (
    select
      coalesce(sum(gross_amount) filter (where type = 'message'), 0) as ppv_gross,
      coalesce(sum(gross_amount) filter (where type = 'tip'), 0) as tip_gross,
      coalesce(sum(gross_amount) filter (where type = 'sub'), 0) as sub_gross,
      coalesce(sum(gross_amount) filter (where type in ('post', 'stream', 'referral')), 0) as other_gross,
      coalesce(sum(gross_amount), 0) as tx_total_gross,
      coalesce(sum(net_amount), 0) as tx_total_net,
      max(created_at) as last_tx_at,
      max(gross_amount) as highest_tx_gross
    from public.model_transactions
    where model_id = p_model_id
      and of_fan_id = p_of_fan_id
      and of_transaction_id not like 'chart:%'
  ),
  ppv_gap as (
    select coalesce(sum(price), 0) as amount,
           max(created_at) as last_at,
           max(price) as highest
    from public.ppv_timeline
    where model_id = p_model_id
      and of_fan_id = p_of_fan_id
      and purchased
      and (select ppv_gross from fan_tx) = 0
  ),
  tip_gap as (
    select coalesce(sum(amount), 0) as amount,
           max(created_at) as last_at,
           max(amount) as highest
    from public.chat_tips
    where model_id = p_model_id
      and of_fan_id = p_of_fan_id
      and (select tip_gross from fan_tx) = 0
  ),
  ppv_seen as (
    select max(created_at) as last_at, max(price) as highest
    from public.ppv_timeline
    where model_id = p_model_id
      and of_fan_id = p_of_fan_id
      and purchased
  ),
  tip_seen as (
    select max(created_at) as last_at, max(amount) as highest
    from public.chat_tips
    where model_id = p_model_id
      and of_fan_id = p_of_fan_id
  )
  select
    case
      when fan_tx.ppv_gross > 0 then fan_tx.ppv_gross
      else greatest(coalesce(f.ppv_total, 0), ppv_gap.amount)
    end as ppv_spent,
    case
      when fan_tx.tip_gross > 0 then fan_tx.tip_gross
      else greatest(coalesce(f.tip_total, 0), tip_gap.amount)
    end as tip_spent,
    coalesce(fan_tx.sub_gross, 0) as sub_spent,
    coalesce(fan_tx.other_gross, 0) as other_spent,
    case
      when f.official_spend then greatest(
        f.total_spend,
        coalesce(fan_tx.tx_total_gross, 0),
        case when fan_tx.ppv_gross = 0 then ppv_gap.amount else 0 end
          + case when fan_tx.tip_gross = 0 then tip_gap.amount else 0 end
      )
      else coalesce(
        nullif(
          coalesce(fan_tx.tx_total_gross, 0)
            + case when fan_tx.ppv_gross = 0 then ppv_gap.amount else 0 end
            + case when fan_tx.tip_gross = 0 then tip_gap.amount else 0 end,
          0
        ),
        f.total_spend
      )
    end as total_spend_gross,
    coalesce(fan_tx.tx_total_net, 0) as total_earned_net,
    (
      select max(stamp)
      from (
        values
          (f.last_spend_at),
          (fan_tx.last_tx_at),
          (ppv_seen.last_at),
          (tip_seen.last_at)
      ) as stamps(stamp)
    ) as last_spend_at,
    greatest(
      coalesce(f.highest_spend, 0),
      coalesce(fan_tx.highest_tx_gross, 0),
      coalesce(ppv_seen.highest, 0),
      coalesce(tip_seen.highest, 0)
    ) as highest_spend
  from public.fans f
  cross join fan_tx
  cross join ppv_gap
  cross join tip_gap
  cross join ppv_seen
  cross join tip_seen
  where f.model_id = p_model_id
    and f.of_fan_id = p_of_fan_id;
$$;

revoke all on function public.refresh_fan_spend_peaks(uuid) from public, anon;
revoke all on function public.fan_spend_breakdown(uuid, text) from public, anon;
grant execute on function public.refresh_fan_spend_peaks(uuid) to authenticated;
grant execute on function public.fan_spend_breakdown(uuid, text) to authenticated;
