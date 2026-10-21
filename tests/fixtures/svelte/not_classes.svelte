<script>
  let a = 1 < 2;
  const html = '<div class="in-script">';
</script>

{#if a < 3}
  <p id="btn" data-class="nope" classes="nope2" title='class="nope3"'>text</p>
{:else if a > 3}
  <p>{html} class="not-in-tag" .btn plain text</p>
{/if}
{@html '<div class="in-html">'}
<!-- <div class="commented-out"> -->
<input disabled {value} {...rest} bind:value on:click={() => count++} use:action />
<p class={a > 1 ? 'big' : 'small'}>after expressions</p>      <!--~ big, small -->
<style>.only-here { color: red }</style>                      <!--~ only-here -->
