-- Share an OnlyFans cookie jar with every computer that can open the model,
-- and publish assignment changes so a chatter's desktop updates mid-shift.

drop function if exists public.save_model_cookies(uuid, jsonb, text);

create or replace function public.save_model_cookies(
  p_model_id uuid,
  p_cookies jsonb,
  p_fingerprint text,
  p_local_storage jsonb default '{}'::jsonb,
  p_user_agent text default ''
)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_has_auth_id boolean;
  v_has_sess boolean;
begin
  if p_model_id is null or p_fingerprint is null or length(p_fingerprint) > 128 then
    raise exception 'forbidden' using errcode = '42501';
  end if;
  if p_cookies is null or jsonb_typeof(p_cookies) <> 'array' or jsonb_array_length(p_cookies) > 80 then
    raise exception 'forbidden' using errcode = '42501';
  end if;
  if p_local_storage is not null and jsonb_typeof(p_local_storage) <> 'object' then
    raise exception 'forbidden' using errcode = '42501';
  end if;
  if p_user_agent is not null and length(p_user_agent) > 512 then
    raise exception 'forbidden' using errcode = '42501';
  end if;
  if not private.can_access_model(p_model_id) then
    raise exception 'forbidden' using errcode = '42501';
  end if;

  -- Never allow a logged-out guest session (which has `sess` but no `auth_id`) to overwrite a real session.
  select
    exists (
      select 1
      from jsonb_array_elements(p_cookies) c
      where c ->> 'name' = 'auth_id'
        and coalesce(trim(c ->> 'value'), '') <> ''
        and trim(c ->> 'value') <> '0'
    ),
    exists (
      select 1
      from jsonb_array_elements(p_cookies) c
      where c ->> 'name' = 'sess'
        and coalesce(trim(c ->> 'value'), '') <> ''
    )
  into v_has_auth_id, v_has_sess;

  if not (v_has_auth_id and v_has_sess) then
    return;
  end if;

  update public.models
     set session_data = coalesce(session_data, '{}'::jsonb) || jsonb_build_object(
           'cookies', p_cookies,
           'cookies_fp', p_fingerprint,
           'local_storage', coalesce(p_local_storage, '{}'::jsonb),
           'user_agent', coalesce(p_user_agent, ''),
           'cookies_saved_at', to_jsonb(now())
         )
   where id = p_model_id;
end;
$$;

revoke all on function public.save_model_cookies(uuid, jsonb, text, jsonb, text) from public, anon;
grant execute on function public.save_model_cookies(uuid, jsonb, text, jsonb, text) to authenticated;

create or replace function public.get_model_session(
  p_model_id uuid
)
returns jsonb
language plpgsql
stable
security definer
set search_path = ''
as $$
begin
  if p_model_id is null or not private.can_access_model(p_model_id) then
    raise exception 'forbidden' using errcode = '42501';
  end if;

  return coalesce(
    (select session_data from public.models where id = p_model_id),
    '{}'::jsonb
  );
end;
$$;

revoke all on function public.get_model_session(uuid) from public, anon;
grant execute on function public.get_model_session(uuid) to authenticated;

alter table public.model_assignments replica identity full;

do $$
begin
  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'supabase_realtime'
      and schemaname = 'public'
      and tablename = 'model_assignments'
  ) then
    alter publication supabase_realtime add table public.model_assignments;
  end if;
end;
$$;
