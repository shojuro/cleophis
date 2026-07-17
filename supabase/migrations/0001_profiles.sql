-- Profiles: one row per auth user, created server-side by trigger.
create table public.profiles (
  id         uuid primary key references auth.users(id) on delete cascade,
  nickname   text not null default 'you' check (char_length(nickname) <= 40),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
alter table public.profiles enable row level security;

create policy "profiles_select_own" on public.profiles
  for select to authenticated using ((select auth.uid()) = id);
create policy "profiles_update_own" on public.profiles
  for update to authenticated using ((select auth.uid()) = id) with check ((select auth.uid()) = id);
-- no INSERT policy: rows are created only by the security-definer trigger

create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer set search_path = ''
as $$
begin
  insert into public.profiles (id, nickname)
  values (new.id,
          coalesce(nullif(new.raw_user_meta_data->>'nickname', ''),
                   nullif(split_part(coalesce(new.email, ''), '@', 1), ''),
                   'you'));
  return new;
end;
$$;

create trigger on_auth_user_created
  after insert on auth.users
  for each row execute function public.handle_new_user();

revoke all on public.profiles from anon;
grant select, update on public.profiles to authenticated;
