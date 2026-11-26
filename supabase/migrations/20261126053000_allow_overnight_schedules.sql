-- Night shifts end after midnight, so the end clock time is earlier than the start.

alter table public.schedules drop constraint if exists schedules_time_check;

alter table public.schedules
  add constraint schedules_time_check check (start_time <> end_time);
