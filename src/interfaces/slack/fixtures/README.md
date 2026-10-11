# Slack wire fixtures — assumed-until-probed

Every fixture in this directory was authored from Slack's published documentation
(Socket Mode and Events API), NOT captured from a live workspace. They are marked
**assumed-until-probed**: at first live deployment, capture real envelopes/events
and replace these files byte-for-byte, then re-run the test suite. Until then,
treat any wire-format bug as possibly ours, not Slack's.
