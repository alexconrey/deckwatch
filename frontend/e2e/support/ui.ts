import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Locators for Vuetify widgets, shared by the specs.
 *
 * The same specs run against Vuetify 3 and the Vuetify 4 upgrade (#242), so
 * they find elements by role, label and text where possible, and otherwise
 * only by component root classes that both versions render (.v-field,
 * .v-card, .v-list-item, .v-overlay--active).
 */

type Scope = Page | Locator;

const pageOf = (scope: Scope): Page => ("page" in scope ? scope.page() : scope);

/**
 * The `.v-field` of the select, autocomplete or combobox labelled `label`.
 * Click this one to open the menu: a v-select's own `<input>` is a single
 * character wide and covered by the selection text.
 */
export function field(scope: Scope, label: string | RegExp): Locator {
  return scope
    .locator(".v-field")
    .filter({ has: pageOf(scope).getByRole("combobox", { name: label, exact: typeof label === "string" }) });
}

/** The list inside the menu that is open on top. */
export function activeMenu(page: Page): Locator {
  return page.locator(".v-overlay--active .v-list").last();
}

/** Open the select-family field labelled `label` and return its menu list. */
export async function openMenu(scope: Scope, label: string | RegExp): Promise<Locator> {
  await field(scope, label).click();
  const menu = activeMenu(pageOf(scope));
  await expect(menu).toBeVisible();
  return menu;
}

/** Pick `option` in the select-family field labelled `label`. */
export async function choose(scope: Scope, label: string | RegExp, option: string | RegExp): Promise<void> {
  const menu = await openMenu(scope, label);
  await menu.getByRole("option", { name: option, exact: typeof option === "string" }).click();
  await expect(menu).toBeHidden();
}

/** An entry of the open menu, by text (list item roles differ between Vuetify versions). */
export function menuItem(page: Page, text: string): Locator {
  return page.locator(".v-overlay--active .v-list-item", { hasText: text });
}

/** The card whose title has an element with exactly this text. */
export function card(page: Page, title: string): Locator {
  return page
    .locator(".v-card")
    .filter({ has: page.locator(".v-card-title").getByText(title, { exact: true }) })
    .first();
}

/** The table row that has a cell with exactly this text. */
export function row(scope: Scope, name: string): Locator {
  return scope.getByRole("row").filter({ has: pageOf(scope).getByRole("cell", { name, exact: true }) });
}

/**
 * The `.v-field` around a select-family control, for fields that share a
 * label (e.g. the Protocol select of each port row).
 */
export function fieldOf(control: Locator): Locator {
  return control.locator('xpath=ancestor::*[contains(concat(" ", normalize-space(@class), " "), " v-field ")][1]');
}
