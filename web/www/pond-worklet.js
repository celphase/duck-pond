// Plays the blocks the pond worker renders. The page sends a MessagePort to
// the worker; blocks arrive on it, and each one taken off the queue is
// acknowledged so the worker renders another. The page is told the context
// time at which each block starts playing, to keep the map in step.

class PondPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.blocks = [];
    this.current = null;
    this.pos = 0;
    this.feed = null;
    this.port.onmessage = ({ data: feed }) => {
      this.feed = feed;
      feed.onmessage = ({ data }) => this.blocks.push(data);
    };
  }

  process(_inputs, outputs) {
    const [left, right] = outputs[0];
    for (let i = 0; i < left.length; i++) {
      let frames = this.current ? this.current.audio.length / 2 : 0;
      if (this.pos >= frames) {
        this.current = this.blocks.shift() ?? null;
        this.pos = 0;
        if (!this.current) {
          // the worker fell behind (or is just starting)
          left.fill(0, i);
          right.fill(0, i);
          break;
        }
        frames = this.current.audio.length / 2;
        this.feed.postMessage(0);
        this.port.postMessage({ frame: this.current.frame, time: (currentFrame + i) / sampleRate });
      }
      left[i] = this.current.audio[this.pos];
      right[i] = this.current.audio[frames + this.pos];
      this.pos++;
    }
    return true;
  }
}

registerProcessor('pond-player', PondPlayer);
