# Next Steps
WIGGUM_REMAINING_WORK=yes

Are there important TODO items not yet captured? No.

- Make `idle_timeout_env_closes_connection` resilient by polling for EOF/close until a deadline and tolerating `TimedOut` reads.
