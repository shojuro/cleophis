-- Security-advisor remediation: the signup trigger function must not be
-- callable through the exposed RPC surface (it's a trigger-only function;
-- direct calls fail anyway, but the surface should not exist).
revoke execute on function public.handle_new_user() from anon, authenticated, public;
