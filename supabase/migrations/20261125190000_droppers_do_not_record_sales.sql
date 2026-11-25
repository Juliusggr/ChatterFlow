-- Droppers keep check-in and check-out, but employee sales belong to chatters.

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
  v_role text;
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
