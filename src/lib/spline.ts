/**
 * Natural cubic spline interpolation for 3D centerline computation.
 *
 * TypeScript port of scipy's CubicSpline with natural boundary conditions
 * (second derivatives = 0 at endpoints).
 */

type Point3D = [number, number, number];

/**
 * Solve a tridiagonal system using the Thomas algorithm.
 *
 * Given n equations of the form:
 *   lower[i]*x[i-1] + diag[i]*x[i] + upper[i]*x[i+1] = rhs[i]
 *
 * Returns the solution vector x.
 */
function solveTridiagonal(
  lower: number[],
  diag: number[],
  upper: number[],
  rhs: number[],
): number[] {
  const n = diag.length;
  // Working copies for forward elimination
  const c = new Float64Array(n);
  const d = new Float64Array(n);
  const x = new Array<number>(n);

  // Forward sweep
  c[0] = upper[0] / diag[0];
  d[0] = rhs[0] / diag[0];
  for (let i = 1; i < n; i++) {
    const m = diag[i] - lower[i] * c[i - 1];
    c[i] = i < n - 1 ? upper[i] / m : 0;
    d[i] = (rhs[i] - lower[i] * d[i - 1]) / m;
  }

  // Back substitution
  x[n - 1] = d[n - 1];
  for (let i = n - 2; i >= 0; i--) {
    x[i] = d[i] - c[i] * x[i + 1];
  }

  return x;
}

/**
 * Fit a natural cubic spline to 1D data and return an evaluator function.
 *
 * @param t - Knot positions (e.g. cumulative arc-length), length n+1
 * @param y - Data values at each knot, length n+1
 * @returns A function that evaluates the spline at parameter s
 */
function fitNaturalCubicSpline(
  t: number[],
  y: number[],
): (s: number) => number {
  const n = t.length - 1; // number of segments

  // Interval widths
  const h = new Array<number>(n);
  for (let i = 0; i < n; i++) {
    h[i] = t[i + 1] - t[i];
  }

  // Build tridiagonal system for interior second derivatives.
  // Natural BCs: c[0] = 0, c[n] = 0, so we solve for c[1]..c[n-1].
  const m = n - 1; // number of interior unknowns
  if (m <= 0) {
    // Linear segment: just lerp
    const slope = (y[1] - y[0]) / h[0];
    return (s: number) => y[0] + slope * (s - t[0]);
  }

  const lower = new Array<number>(m).fill(0);
  const diag = new Array<number>(m).fill(0);
  const upper = new Array<number>(m).fill(0);
  const rhs = new Array<number>(m).fill(0);

  for (let j = 0; j < m; j++) {
    const i = j + 1; // index into original arrays
    lower[j] = j > 0 ? h[i - 1] : 0;
    diag[j] = 2 * (h[i - 1] + h[i]);
    upper[j] = j < m - 1 ? h[i] : 0;
    rhs[j] =
      3 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
  }

  const cInterior = solveTridiagonal(lower, diag, upper, rhs);

  // Full second-derivative coefficients (c[0] = 0, c[n] = 0)
  const c = new Array<number>(n + 1).fill(0);
  for (let j = 0; j < m; j++) {
    c[j + 1] = cInterior[j];
  }

  // Compute b and d coefficients for each segment
  const a = y; // a[i] = y[i]
  const b = new Array<number>(n);
  const d = new Array<number>(n);
  for (let i = 0; i < n; i++) {
    b[i] =
      (y[i + 1] - y[i]) / h[i] - (h[i] * (2 * c[i] + c[i + 1])) / 3;
    d[i] = (c[i + 1] - c[i]) / (3 * h[i]);
  }

  // Return evaluator: S_i(s) = a[i] + b[i]*(s-t[i]) + c[i]*(s-t[i])^2 + d[i]*(s-t[i])^3
  return (s: number): number => {
    // Clamp to valid range
    if (s <= t[0]) return a[0];
    if (s >= t[n]) return a[n];

    // Binary search for the correct segment
    let lo = 0;
    let hi = n - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (s > t[mid + 1]) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    const i = lo;
    const ds = s - t[i];
    return a[i] + ds * (b[i] + ds * (c[i] + ds * d[i]));
  };
}

/**
 * Euclidean distance between two 3D points.
 */
function dist3D(a: Point3D, b: Point3D): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const dz = b[2] - a[2];
  return Math.sqrt(dx * dx + dy * dy + dz * dz);
}

/**
 * Compute a dense 3D centerline by fitting a natural cubic spline through
 * the given seed points, parameterized by cumulative arc-length.
 *
 * @param points - Ordered 3D coordinates in mm (ostium first, then waypoints)
 * @param stepMm - Sampling interval along the spline, default 0.5 mm
 * @returns Dense array of 3D points sampled at uniform arc-length intervals
 */
export function computeSplineCenterline(
  points: Point3D[],
  stepMm: number = 0.5,
): Point3D[] {
  if (points.length < 2) return [];

  // Cumulative arc-length parameter
  const arcLen = [0];
  for (let i = 1; i < points.length; i++) {
    arcLen.push(arcLen[i - 1] + dist3D(points[i - 1], points[i]));
  }
  const totalLength = arcLen[arcLen.length - 1];
  if (totalLength < 1e-12) return [points[0]];

  // For exactly 2 points, linearly interpolate
  if (points.length === 2) {
    const result: Point3D[] = [];
    const numSteps = Math.floor(totalLength / stepMm);
    for (let k = 0; k <= numSteps; k++) {
      const frac = k / numSteps;
      result.push([
        points[0][0] + frac * (points[1][0] - points[0][0]),
        points[0][1] + frac * (points[1][1] - points[0][1]),
        points[0][2] + frac * (points[1][2] - points[0][2]),
      ]);
    }
    return result;
  }

  // Fit independent splines for x, y, z using arc-length as parameter
  const xVals = points.map((p) => p[0]);
  const yVals = points.map((p) => p[1]);
  const zVals = points.map((p) => p[2]);

  const evalX = fitNaturalCubicSpline(arcLen, xVals);
  const evalY = fitNaturalCubicSpline(arcLen, yVals);
  const evalZ = fitNaturalCubicSpline(arcLen, zVals);

  // Sample at uniform arc-length intervals
  const numSteps = Math.max(1, Math.round(totalLength / stepMm));
  const result: Point3D[] = [];
  for (let k = 0; k <= numSteps; k++) {
    const s = (k / numSteps) * totalLength;
    result.push([evalX(s), evalY(s), evalZ(s)]);
  }

  return result;
}
