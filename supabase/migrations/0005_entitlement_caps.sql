-- Pre-payments hardening: bound client-writable entitlement rows.
-- model_id gets a length cap; each user gets a row-count cap. Both close
-- junk-row floods while inserts remain client-open for trial/library.
alter table public.entitlements
  add constraint entitlements_model_id_length
  check (char_length(model_id) between 1 and 64);

-- Soft cap: unlocked count check — concurrent inserts can exceed it slightly; it is flood control, not an invariant. Rows that already exist (UPSERT updates) are exempt.
create or replace function public.enforce_entitlement_cap()
returns trigger
language plpgsql
security definer set search_path = ''
as $$
begin
  if not exists (
    select 1 from public.entitlements
    where user_id = new.user_id and model_id = new.model_id
  ) and (select count(*) from public.entitlements where user_id = new.user_id) >= 100 then
    raise exception 'entitlement limit reached';
  end if;
  return new;
end;
$$;

create trigger entitlements_cap_check
  before insert on public.entitlements
  for each row execute function public.enforce_entitlement_cap();

revoke execute on function public.enforce_entitlement_cap() from anon, authenticated, public;
