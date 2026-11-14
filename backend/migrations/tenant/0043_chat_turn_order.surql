-- A token fences stale writers after cancellation, process death or lease expiry.
DEFINE FIELD IF NOT EXISTS turn_token ON chat_thread TYPE option<string>;
DEFINE FIELD IF NOT EXISTS turn_until ON chat_thread TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS history_at ON chat_thread TYPE option<datetime>;
