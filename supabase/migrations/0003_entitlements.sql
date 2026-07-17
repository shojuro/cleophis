-- Entitlements: what a user may download/run. Clients may self-grant only
-- trial/library; 'purchase' is reserved for the service_role (Stripe webhook,
-- Milestone B) so revenue data is trustworthy from day one.
create type public.entitlement_source as enum ('trial', 'purchase', 'library');

create table public.entitlements (
  id         uuid primary key default gen_random_uuid(),
  user_id    uuid not null references auth.users(id) on delete cascade,
  model_id   text not null,
  source     public.entitlement_source not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz,
  unique (user_id, model_id)
);
alter table public.entitlements enable row level security;

create policy "entitlements_select_own" on public.entitlements
  for select to authenticated using ((select auth.uid()) = user_id);
create policy "entitlements_insert_own_nonpurchase" on public.entitlements
  for insert to authenticated
  with check ((select auth.uid()) = user_id and source in ('trial','library'));
-- no UPDATE/DELETE policies: clients cannot revoke or extend

create index entitlements_user_id_idx on public.entitlements (user_id);
revoke all on public.entitlements from anon;
grant select, insert on public.entitlements to authenticated;
