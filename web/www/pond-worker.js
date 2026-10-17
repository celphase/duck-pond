// Runs the pond (duck_pond.wasm) off the main thread, keeping the audio
// worklet a few blocks ahead, as the native player's scene thread does.
//
// Each block goes to the worklet tagged with its frame, and a snapshot of the
// scene at that frame goes to the page, which shows it once the worklet says
// that frame is being heard.

/** Blocks rendered ahead of playback (about 170 ms at 48 kHz), enough to
 *  ride out a long call being synthesized on a slow phone. */
const QUEUED = 16;

const decoder = new TextDecoder();
let pond = null;
let feed = null;
let block = 0;
let queued = 0;

onmessage = async ({ data }) => {
  switch (data.type) {
    case 'start':
      try {
        const response = await fetch('duck_pond.wasm');
        if (!response.ok) throw new Error(`couldn't load duck_pond.wasm (${response.status})`);
        const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
        pond = instance.exports;
        block = pond.pond_block();
        pond.pond_new(data.sampleRate, data.seed[0], data.seed[1], data.headphones ? 1 : 0);
        feed = data.feed;
        // the worklet has taken a block off its queue
        feed.onmessage = () => {
          queued--;
          pump();
        };
        pump();
      } catch (err) {
        postMessage({ type: 'error', message: String(err.message ?? err) });
      }
      break;
    case 'peas':
      pond?.pond_scatter_peas();
      break;
    case 'headphones':
      pond?.pond_set_headphones(data.on ? 1 : 0);
      break;
  }
};

function pump() {
  while (queued < QUEUED) {
    const len = pond.pond_snapshot();
    const json = decoder.decode(new Uint8Array(pond.memory.buffer, pond.pond_snapshot_ptr(), len));
    const snapshot = JSON.parse(json);
    // rendering may grow the module's memory, replacing its buffer, so look
    // for it only afterwards, and copy the block out
    const ptr = pond.pond_render();
    const audio = new Float32Array(pond.memory.buffer, ptr, 2 * block).slice();
    feed.postMessage({ frame: snapshot.frame, audio }, [audio.buffer]);
    postMessage({ type: 'snapshot', snapshot });
    queued++;
  }
}
