-- Security hardening: the entitlement cap must never block a purchase write.
-- The 0005 cap counted ALL rows and fired on any new (user,model) insert,
-- so a user sitting at 100 self-inserted trial/library rows who then bought
-- a new model tripped the trigger on the service-role INSERT — paid, but the
-- webhook 500'd and the entitlement was never granted (no client DELETE to
-- self-remedy). Cap only the client-writable sources, and count only those.
create or replace function public.enforce_entitlement_cap()
returns trigger
language plpgsql
security definer set search_path = ''
as $$
begin
  if new.source in ('trial', 'library')
     and not exists (
       select 1 from public.entitlements
       where user_id = new.user_id and model_id = new.model_id
     )
     and (
       select count(*) from public.entitlements
       where user_id = new.user_id and source in ('trial', 'library')
     ) >= 100 then
    raise exception 'entitlement limit reached';
  end if;
  return new;
end;
$$;

revoke execute on function public.enforce_entitlement_cap() from anon, authenticated, public;
