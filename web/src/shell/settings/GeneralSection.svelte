<script lang="ts">
  import { isTimeZoneName, timeZoneChoices } from "../../lib/time-zones";
  import { Button, Disclosure, SelectField, TextField } from "../../lib/ui";
  import ConfigNumber from "./ConfigNumber.svelte";
  import { fieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // General: the timezone every agent shares, and under More options the
  // address Residuum serves this app on. Both are staged and saved with the
  // rest of the install-wide settings.

  let { scope, section }: AllSectionProps = $props();

  const deviceZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const zoneChoices = $derived(timeZoneChoices(scope.config.timezone));

  const zone = $derived(scope.config.timezone.trim());
  // A flag, not a block: the save is still the server's to refuse. The
  // dropdown only offers real names, so this is a value already in the file.
  const zoneError = $derived(
    fieldError(scope, { kind: "config", field: "timezone" }) ??
      (zone !== "" && !isTimeZoneName(zone)
        ? "That doesn't look like a timezone name, so Residuum may refuse it. Names look like Europe/Berlin."
        : undefined),
  );
  const bindError = $derived(fieldError(scope, { kind: "config", field: "gateway_bind" }));
  const portError = $derived(fieldError(scope, { kind: "config", field: "gateway_port" }));

  let moreOpen = $state(false);
  $effect(() => {
    if (bindError !== undefined || portError !== undefined) moreOpen = true;
  });
</script>

<SettingsSection
  {scope}
  {section}
  title="General"
  lede="Your timezone, and where Residuum listens. Applies to every agent."
>
  <SettingsGroup>
    <SelectField
      label="Timezone"
      bind:value={scope.config.timezone}
      options={zoneChoices.ungrouped}
      groups={zoneChoices.groups}
      placeholder="Choose a timezone"
      hint="Used for every agent's schedules, quiet hours and timestamps. Changing it later doesn't convert timestamps already stored."
      error={zoneError}
    />
    {#if deviceZone !== "" && zone !== deviceZone}
      <div>
        <Button
          variant="quiet"
          size="sm"
          onclick={() => {
            scope.config.timezone = deviceZone;
          }}
        >
          Use this device's timezone ({deviceZone})
        </Button>
      </div>
    {/if}
  </SettingsGroup>

  <Disclosure summary="More options" bind:open={moreOpen}>
    <SettingsGroup
      title="Where Residuum listens"
      lede="Saving moves Residuum to the new address straight away, so after changing the port you need to reopen Residuum there."
    >
      <TextField
        label="Bind address"
        bind:value={scope.config.gateway_bind}
        placeholder="127.0.0.1"
        autocomplete="off"
        spellcheck={false}
        code
        hint="127.0.0.1 keeps Residuum on this machine. 0.0.0.0 lets other devices on your network reach it."
        error={bindError}
      />
      <ConfigNumber
        {scope}
        field="gateway_port"
        label="Port"
        placeholder="7700"
        min={1}
        max={65535}
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>
