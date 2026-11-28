-- The same PPV message was stored twice: once when the chat marked it opened
-- and again when the Compras notification arrived, under a different id.
-- A second PPV for the same fan and price inside 2 minutes is that duplicate.

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
  v_fan text := coalesce(p_fan_platform_id, '');
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
  if p_category = 'ppv' and exists (
    select 1
    from public.sales as existing
    where existing.employee_id = v_user
      and existing.model_id = v_model
      and existing.fan_platform_id = v_fan
      and existing.category = 'ppv'
      and existing.amount_gross = p_amount_gross
      and abs(extract(epoch from (existing.recorded_at - v_event_at))) < 120
  ) then
    return null;
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
    v_fan,
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

delete from public.sales as later
where later.category = 'ppv'
  and exists (
    select 1
    from public.sales as earlier
    where earlier.id <> later.id
      and earlier.employee_id = later.employee_id
      and earlier.model_id = later.model_id
      and earlier.fan_platform_id = later.fan_platform_id
      and earlier.category = 'ppv'
      and earlier.amount_gross = later.amount_gross
      and earlier.recorded_at < later.recorded_at
      and later.recorded_at - earlier.recorded_at < interval '2 minutes'
  );
