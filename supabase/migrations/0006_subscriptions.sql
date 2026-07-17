-- Subscriptions: Stripe customer mapping + the single subscription writer.
-- stripe_customers is written only by the stripe-webhook (service role);
-- clients get no access at all (portal minting is server-side).
create table public.stripe_customers (
  user_id     uuid primary key references auth.users(id) on delete cascade,
  customer_id text not null unique,
  created_at  timestamptz not null default now()
);
alter table public.stripe_customers enable row level security;
-- No policies: service_role bypasses RLS; authenticated/anon get nothing.
revoke all on table public.stripe_customers from anon, authenticated, public;

-- Monotonic, lifetime-protecting subscription period writer. The ONLY code
-- allowed to set a non-null expires_at. One write direction (extend), which
-- makes Stripe webhook replays and out-of-order deliveries harmless.
create or replace function public.apply_subscription_period(
  p_user_id uuid, p_model_id text, p_period_end timestamptz
) returns void
language plpgsql security definer set search_path = ''
as $$
begin
  -- NULL period_end would mint an un-expirable row indistinguishable from a
  -- grandfathered lifetime purchase — fail loudly instead.
  if p_period_end is null then
    raise exception 'apply_subscription_period: null period_end';
  end if;
  insert into public.entitlements (user_id, model_id, source, expires_at)
  values (p_user_id, p_model_id, 'purchase', p_period_end)
  on conflict (user_id, model_id) do update
    set source = 'purchase',
        expires_at = case
          -- Grandfathered lifetime purchase (purchase + null expiry): never touched.
          -- The explicit branch matters: GREATEST() ignores NULLs and would
          -- otherwise stamp a date onto a lifetime row.
          when entitlements.source = 'purchase' and entitlements.expires_at is null
            then null
          -- Client trial/library rows are always null-expiry (RLS-forced):
          -- upgrading them takes the incoming period.
          when entitlements.expires_at is null
            then excluded.expires_at
          -- Renewal / replay / out-of-order: extend only, never shrink.
          else greatest(entitlements.expires_at, excluded.expires_at)
        end;
end;
$$;
revoke execute on function public.apply_subscription_period(uuid, text, timestamptz)
  from anon, authenticated, public;
