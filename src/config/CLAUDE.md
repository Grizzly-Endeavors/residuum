# Config Module

Any addition or removal of configuration options in this module **must** be reflected in the Settings modal in `web/`: the section components in `web/src/shell/settings/`, and the field-to-key-path map in `web/src/lib/settings-fields.ts`. The Settings modal is the primary way users interact with configuration — keeping them in sync is mandatory.
