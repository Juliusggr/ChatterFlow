-- Only an active owner can delete shift rows. Sales recorded during the shift stay.

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

create policy shifts_delete_owner on public.shifts
  for delete to authenticated
  using ((select private.is_owner()));

grant delete on public.shifts to authenticated;
