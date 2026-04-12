<script lang="ts">
  const tooltip =
    "These messages were compressed by the observer. The agent remembers their substance but not the exact wording.";
</script>

<div class="msg msg-compressed-marker" role="note" aria-label="compressed history boundary">
  <span class="msg-divider-line" aria-hidden="true"></span>
  <span class="compressed-marker-label">
    compressed history
    <button
      type="button"
      class="compressed-marker-tooltip-icon"
      aria-label="What is compressed history?">?</button
    >
    <span class="compressed-marker-tooltip-text" role="tooltip">{tooltip}</span>
  </span>
  <span class="msg-divider-line" aria-hidden="true"></span>
</div>
