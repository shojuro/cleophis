-- Storage-abuse hardening: bound client-writable device rows, mirroring
-- entitlements' 0005/0007 caps. fingerprint/gpu/platform get length caps;
-- each user gets a row-count cap. Devices are upserted by (user_id,
-- fingerprint) after online auth (see 0002), so existing rows (UPSERT
-- updates) are exempt from the count -- only genuinely new devices count
-- toward the cap.
alter table public.devices
  add constraint devices_fingerprint_length
  check (char_length(fingerprint) between 1 and 256);
alter table public.devices
  add constraint devices_gpu_length
  check (char_length(gpu) <= 256);
alter table public.devices
  add constraint devices_platform_length
  check (char_length(platform) <= 256);

-- Soft cap: unlocked count check -- concurrent inserts can exceed it
-- slightly; it is flood control, not an invariant. A real user has a
-- handful of devices (desktop, laptop, workstation, ...); 50 is generous
-- headroom while still bounding an automated storage/cost-abuse flood.
create or replace function public.enforce_device_cap()
returns trigger
language plpgsql
security definer set search_path = ''
as $$
begin
  if not exists (
    select 1 from public.devices
    where user_id = new.user_id and fingerprint = new.fingerprint
  ) and (select count(*) from public.devices where user_id = new.user_id) >= 50 then
    raise exception 'device limit reached';
  end if;
  return new;
end;
$$;

create trigger devices_cap_check
  before insert on public.devices
  for each row execute function public.enforce_device_cap();

revoke execute on function public.enforce_device_cap() from anon, authenticated, public;
