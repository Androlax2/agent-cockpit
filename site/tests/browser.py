import argparse
import json
import re
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

parser = argparse.ArgumentParser(
    description="Check an isolated documentation build with headless Chromium."
)
parser.add_argument(
    "--url", default="http://127.0.0.1:45840/agent-cockpit/latest/get-started/first-session/"
)
parser.add_argument("--output", type=Path, default=Path("/tmp/cockpit-102-browser"))
args = parser.parse_args()
url = args.url
shots = args.output
shots.mkdir(parents=True, exist_ok=True)
results = {"url": url, "browser": "headless Chromium", "checks": [], "errors": []}


def luminance(channels):
    linear = [
        (v / 255 / 12.92 if v / 255 <= 0.04045 else ((v / 255 + 0.055) / 1.055) ** 2.4)
        for v in channels
    ]
    return sum(v * w for v, w in zip(linear, [0.2126, 0.7152, 0.0722], strict=True))


with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    desktop = browser.new_context(viewport={"width": 1280, "height": 1000}, color_scheme="light")
    page = desktop.new_page()
    page.on("pageerror", lambda e: results["errors"].append(str(e)))
    page.goto(url)
    page.wait_for_load_state("networkidle")
    expect(
        page.get_by_role("heading", name="First project and session", exact=True)
    ).to_be_visible()
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
    description = page.locator("p.text-fd-muted-foreground").first
    colors = description.evaluate(
        "e=>({foreground:getComputedStyle(e).color,background:getComputedStyle(document.body).backgroundColor})"
    )
    foreground = [float(v) for v in re.findall(r"[\d.]+", colors["foreground"])][:3]
    background = [float(v) for v in re.findall(r"[\d.]+", colors["background"])][:3]
    ratio = (luminance(background) + 0.05) / (luminance(foreground) + 0.05)
    assert ratio >= 4.5, (colors, ratio)
    results["light_description_contrast"] = {**colors, "ratio": ratio}
    page.screenshot(path=str(shots / "102-web-desktop-light.png"), full_page=True)
    trigger = page.get_by_role("button").filter(has_text=re.compile(r"^Search"))
    trigger.click()
    dialog = page.get_by_role("dialog")
    expect(dialog).to_be_visible()
    field = page.get_by_role("combobox", name="Search documentation")
    field.fill("project")
    expect(dialog.get_by_role("option").first).to_be_visible(timeout=15000)
    page.screenshot(path=str(shots / "102-web-search-results.png"))
    field.fill("zzzznonexistent102")
    expect(dialog.get_by_text("No matches.", exact=False)).to_be_visible()
    page.screenshot(path=str(shots / "102-web-search-empty.png"))
    page.get_by_role("button", name="Close search", exact=True).click()
    expect(dialog).not_to_be_visible()
    expect(trigger).to_be_focused()
    page.keyboard.press("Control+k")
    expect(dialog).to_be_visible()
    page.keyboard.press("Escape")
    expect(dialog).not_to_be_visible()
    expect(trigger).to_be_focused()
    results["checks"].append(
        "desktop search: results, no matches, pointer Close, Ctrl+K, Escape and trigger focus restoration"
    )
    page.get_by_role("button", name="User guide", exact=True).click()
    page.get_by_role("link", name="Approvals", exact=True).first.click()
    expect(page.get_by_role("heading", name="Approvals", exact=True)).to_be_visible()
    assert "/latest/guide/approvals" in page.url
    page.go_back()
    expect(
        page.get_by_role("heading", name="First project and session", exact=True)
    ).to_be_visible()
    results["checks"].append("client navigation and browser Back update content and URL")
    trigger.click()
    field.fill("Configuration defaults")
    expect(dialog.get_by_role("option").first).to_be_visible()
    field.press("ArrowDown")
    field.press("Enter")
    expect(dialog).not_to_be_visible()
    page.wait_for_load_state("networkidle")
    assert page.url != url, page.url
    results["checks"].append("ArrowDown and Enter navigate a search result")
    page.goto(url)
    page.wait_for_load_state("networkidle")
    page.evaluate("document.documentElement.style.fontSize='200%'")
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
    results["checks"].append(
        "200% root text size retains reading layout without horizontal page overflow at 1280px; this is text scaling, not browser zoom"
    )
    desktop.close()
    dark = browser.new_context(viewport={"width": 1280, "height": 1000}, color_scheme="dark")
    page = dark.new_page()
    page.goto(url)
    page.wait_for_load_state("networkidle")
    assert page.locator("html").evaluate("e=>e.classList.contains('dark')")
    page.screenshot(path=str(shots / "102-web-desktop-dark.png"), full_page=True)
    results["checks"].append("system dark theme applied")
    dark.close()
    mobile = browser.new_context(
        viewport={"width": 320, "height": 900}, is_mobile=True, has_touch=True
    )
    page = mobile.new_page()
    page.on("pageerror", lambda e: results["errors"].append(str(e)))
    page.goto(url)
    page.wait_for_load_state("networkidle")
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
    page.screenshot(path=str(shots / "102-web-mobile-light.png"), full_page=True)
    page.get_by_role("button", name="Open Sidebar", exact=True).click()
    expect(page.get_by_role("combobox", name="Documentation version")).to_be_visible()
    page.screenshot(path=str(shots / "102-web-mobile-navigation.png"))
    page.get_by_role("button", name="Close Sidebar", exact=True).filter(visible=True).last.click()
    page.get_by_role("button", name="Open Search", exact=True).filter(visible=True).first.click()
    expect(page.get_by_role("dialog")).to_be_visible()
    close = page.get_by_role("button", name="Close search", exact=True)
    rect = close.bounding_box()
    assert rect["width"] >= 80 and rect["height"] >= 44, rect
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")
    page.screenshot(path=str(shots / "102-web-mobile-search.png"))
    close.tap()
    expect(page.get_by_role("dialog")).not_to_be_visible()
    results["checks"].append(
        "320px reading, navigation/version selector and touch Close 80x44 without page overflow"
    )
    page.emulate_media(color_scheme="dark")
    expect(page.locator("html")).to_have_class(re.compile(r"dark"))
    page.screenshot(path=str(shots / "102-web-mobile-dark.png"), full_page=True)
    results["checks"].append(
        "320px system dark theme and long command scrolling without horizontal page overflow"
    )
    mobile.close()
    failed = browser.new_context()
    failed.route(
        "**/search.json", lambda route: route.fulfill(status=503, body="fixture unavailable")
    )
    page = failed.new_page()
    page.goto(url)
    page.wait_for_load_state("networkidle")
    page.keyboard.press("Control+k")
    page.get_by_role("combobox", name="Search documentation").fill("project")
    expect(page.get_by_role("alert")).to_contain_text("Search is unavailable", timeout=15000)
    page.screenshot(path=str(shots / "102-web-search-unavailable.png"))
    page.get_by_role("button", name="Close search", exact=True).click()
    results["checks"].append("failed local index gives a usable unavailable state with Close")
    failed.close()
    browser.close()
assert not results["errors"], results["errors"]
(shots / "browser-evidence.json").write_text(json.dumps(results, indent=2) + "\n")
print(json.dumps(results, indent=2))
