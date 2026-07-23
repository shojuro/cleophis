// The calc() tool schema handed to llama-server on every completion. The
// description carries the two rules the evaluator enforces: radians+deg, and
// no percent operator. Keep in sync with crates/kpack-calc.
export const CALC_TOOL = {
  type: 'function',
  function: {
    name: 'calc',
    description:
      'Evaluate an arithmetic or scientific expression and return the exact result. ' +
      'Use this for EVERY calculation; never compute numbers yourself. ' +
      'Supports + - * / ^, sqrt, sin, cos, tan, log (base 10), ln, abs, round, and pi, e. ' +
      'Trig is in RADIANS — write "30 deg" for degrees, e.g. sin(30 deg). ' +
      'There is no percent or modulo operator; for a percentage compute (x/100)*y.',
    parameters: {
      type: 'object',
      properties: {
        expression: { type: 'string', description: 'e.g. "(3/4)*88" or "sqrt(144)+5"' },
      },
      required: ['expression'],
    },
  },
};
