-- SQL functions that only return void can be inlined and then skipped.
-- plpgsql keeps the fan update from being optimized away.

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

revoke all on function public.refresh_fan_spend_peaks(uuid) from public, anon;
grant execute on function public.refresh_fan_spend_peaks(uuid) to authenticated;
