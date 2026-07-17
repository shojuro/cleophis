-- Pre-payments hardening: bound client-writable entitlement rows.
-- model_id gets a length cap; each user gets a row-count cap. Both close
-- junk-row floods while inserts remain client-open for trial/library.
alter table public.entitlements
  add constraint entitlements_model_id_length
  check (char_length(model_id) between 1 and 64);

create or replace function public.enforce_entitlement_cap()
returns trigger
language plpgsql
security definer set search_path = ''
as $$
begin
  if (select count(*) from public.entitlements where user_id = new.user_id) >= 100 then
    raise exception 'entitlement limit reached';
  end if;
  return new;
end;
$$;

create trigger entitlements_cap_check
  before insert on public.entitlements
  for each row execute function public.enforce_entitlement_cap();

revoke execute on function public.enforce_entitlement_cap() from anon, authenticated, public;
