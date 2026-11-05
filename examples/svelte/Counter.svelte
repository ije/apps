<script lang="ts">
  let { step }: { step: number } = $props()
  let count = $state(0)
  let doubled = $derived(count * 2)
</script>

<section>
  <h2>Count: {count}</h2>
  {#if count > 0}
    <p>Doubled: {doubled}</p>
  {/if}
  <div>
    <button onclick={() => count += step}>Add {step}</button>
    <button class="secondary" onclick={() => count = 0}>Reset</button>
  </div>
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 24px;
    border-radius: 12px;
    background: white;
    box-shadow: 0 4px 12px rgb(0 0 0 / 0.08);
  }
  h2, p {
    margin: 0;
  }
  div {
    display: flex;
    gap: 12px;
  }
  button {
    padding: 10px 20px;
    border: 0;
    border-radius: 8px;
    background: #ff5a24;
    color: white;
    font-size: 16px;
    font-weight: 600;
    cursor: pointer;
  }
  .secondary {
    background: #fff4ed;
    color: #3b2118;
  }
</style>
