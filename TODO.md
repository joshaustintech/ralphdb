# Next Steps
WIGGUM_REMAINING_WORK=yes

Are there important TODO items not yet captured? No.

- Make `idle_timeout_env_closes_connection` resilient by polling for EOF/close until a deadline and tolerating `TimedOut` reads.
- Stop setting `set_read_timeout` on the writer socket clone; keep read timeout on the reader and (optionally) keep write timeout on the writer.
