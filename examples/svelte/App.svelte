<script lang="ts">
  import Counter from './Counter.svelte'

  let name = $state('Svelte')
</script>

<main>
  <p class="label">SVELTE COMPONENTS</p>
  <h1>Hello, {name}!</h1>
  <p class="muted">A package.json loader compiles .svelte files with svelte/compiler.</p>
  <input bind:value={name} aria-label="Your name" placeholder="Your name">
  <Counter step={2} />
</main>

<style>
  :global(body) {
    margin: 0;
    background: #fff4ed;
    color: #3b2118;
    font-family: system-ui, sans-serif;
  }
  main {
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 640px;
    margin: 0 auto;
    padding: 48px 24px;
  }
  h1 {
    margin: 0;
    font-size: 44px;
  }
  .label {
    margin: 0;
    font-size: 12px;
    font-weight: 700;
  }
  .muted {
    margin: 0;
    color: #80665a;
  }
  input {
    padding: 10px 12px;
    border: 1px solid #e5cec0;
    border-radius: 8px;
    font-size: 16px;
  }
</style>
