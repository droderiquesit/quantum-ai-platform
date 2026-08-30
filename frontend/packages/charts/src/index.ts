/**
 * Algorik chart primitives — hand-drawn SVG, no charting library.
 *
 * Three rules hold across every mark, because a chart on a trading surface is
 * read at a glance and a wrong glance is worse than no chart: a series of one
 * point is not a line, the domain is stated rather than silently inferred, and
 * colour means direction. See the implementation for how each is enforced.
 */
export {
  Sparkline, AreaChart, Gauge, Bars, Heatmap,
  type SparklineProps, type AreaChartProps, type GaugeProps, type BarsProps, type HeatCell,
} from "../../../frontend/src/components/viz/primitives";
