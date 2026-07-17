-- Devices: hardware profiles per user, upserted after online auth.
create table public.devices (
  id          uuid primary key default gen_random_uuid(),
  user_id     uuid not null references auth.users(id) on delete cascade,
  fingerprint text not null,
  gpu         text,
  ram_gb      int,
  vram_gb     int,
  platform    text,
  tier        text check (tier in ('high','mid','low')),
  last_seen   timestamptz not null default now(),
  created_at  timestamptz not null default now(),
  unique (user_id, fingerprint)
);
alter table public.devices enable row level security;

create policy "devices_select_own" on public.devices
  for select to authenticated using ((select auth.uid()) = user_id);
create policy "devices_insert_own" on public.devices
  for insert to authenticated with check ((select auth.uid()) = user_id);
create policy "devices_update_own" on public.devices
  for update to authenticated using ((select auth.uid()) = user_id) with check ((select auth.uid()) = user_id);

create index devices_user_id_idx on public.devices (user_id);
revoke all on public.devices from anon;
grant select, insert, update on public.devices to authenticated;
