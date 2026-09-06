/**
 * Every destination in the console's map has an icon of its own.
 *
 * The failure this prevents has already happened once: the Treasury section
 * shipped four items with no `ITEM_ICON` entry, and because the sidebar falls
 * back to the dashboard glyph for an unmapped href, all four rendered wearing
 * the executive dashboard's icon and nothing complained. The fallback is what
 * makes this untestable from the DOM — every item renders *an* svg — so the
 * assertion is on the map itself, and the premise is asserted first so a map
 * that resolved to nothing could not pass by iterating zero items.
 */
import { expect, test } from "@playwright/test";
import { ITEM_ICON } from "../src/components/chrome/icons";
import { NAV_ITEMS } from "../src/lib/nav";

test("the map under test is the console's own and is not empty", () => {
  expect(NAV_ITEMS.length).toBeGreaterThan(30);
  expect(NAV_ITEMS.map((item) => item.href)).toContain("/treasury/ledger");
  expect(NAV_ITEMS.map((item) => item.href)).toContain("/cognition/self-model");
});

test("every nav item has an icon of its own in the item icon map", () => {
  const unmapped = NAV_ITEMS.filter((item) => ITEM_ICON[item.href] === undefined).map((item) => item.href);
  expect(unmapped, `nav items with no ITEM_ICON entry, so they would wear the fallback glyph: ${unmapped.join(", ")}`).toEqual([]);
});
