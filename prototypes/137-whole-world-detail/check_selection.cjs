// PROTOTYPE (issue #137): adversarial check of the selection model embedded in selection.html.
// Run: node prototypes/137-whole-world-detail/check_selection.cjs
const fs = require("fs");
const path = require("path");
const html = fs.readFileSync(path.join(__dirname, "selection.html"), "utf8");
const source = html.match(/<script id="selection-model">([\s\S]*?)<\/script>/)[1];
const loaded = { exports: null };
new Function("module", source)(loaded);
const M = loaded.exports;

let seed = 137;
const random = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
const SIZE = M.GRID * M.EDGE;
const clamp = (value) => Math.min(Math.max(value, 0), SIZE - 1e-3);

for (const configuration of ["demo", "qualification"]) {
  const maxima = M.maxima(configuration);
  const observed = [0, 0, 0, 0];
  let failures = 0, centreMoves = 0, minimumReversalTravel = Infinity, teleports = 0;
  for (let walk = 0; walk < 2000; walk++) {
    let state = M.initial(configuration, [random() * SIZE, random() * SIZE]);
    let axisTravel = [0, 0], axisPrevious = [null, null];
    for (let step = 0; step < 2000; step++) {
      const kind = random();
      let next;
      if (kind < 0.01) {
        next = [random() * SIZE, random() * SIZE];
      } else if (kind < 0.5) {
        // Boundary wobble: small steps that keep reversing near volume faces.
        const length = (random() - 0.5) * 48;
        const axis = random() < 0.5 ? 0 : 1;
        next = state.camera.slice();
        next[axis] = clamp(next[axis] + length);
        if (random() < 0.3) next[1 - axis] = clamp(next[1 - axis] + length);
      } else {
        const angle = random() * Math.PI * 2, length = random() * 64;
        next = [clamp(state.camera[0] + Math.cos(angle) * length), clamp(state.camera[1] + Math.sin(angle) * length)];
      }
      const displacement = Math.hypot(next[0] - state.camera[0], next[1] - state.camera[1]);
      const before = state.centre.slice(), previousCamera = state.camera.slice();
      state = M.accept(state, next);
      state.counts.forEach((count, level) => {
        observed[level] = Math.max(observed[level], count);
        if (count > maxima[level]) failures++;
      });
      // The eye stays inside the centre volume grown by the deadband, so full detail reaches reach*64-16.
      for (let axis = 0; axis < 2; axis++) {
        const low = state.centre[axis] * M.EDGE - M.DEADBAND, high = (state.centre[axis] + 1) * M.EDGE + M.DEADBAND;
        if (state.camera[axis] < low || state.camera[axis] >= high) failures++;
      }
      if (displacement > 64) { teleports++; axisTravel = [0, 0]; axisPrevious = [null, null]; continue; }
      // Churn check: returning an axis's centre to the index it just left needs 32 units of travel on that axis.
      for (let axis = 0; axis < 2; axis++) {
        axisTravel[axis] += Math.abs(next[axis] - previousCamera[axis]);
        if (before[axis] === state.centre[axis]) continue;
        centreMoves++;
        if (state.centre[axis] === axisPrevious[axis]) {
          minimumReversalTravel = Math.min(minimumReversalTravel, axisTravel[axis]);
          if (axisTravel[axis] < 2 * M.DEADBAND) failures++;
        }
        axisPrevious[axis] = before[axis];
        axisTravel[axis] = 0;
      }
    }
  }
  console.log(JSON.stringify({ configuration, maxima, observed, failures, centreMoves, teleports, minimumReversalTravel }));
}
