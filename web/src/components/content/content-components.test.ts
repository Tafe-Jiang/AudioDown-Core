import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  api,
  type ContentFailure,
  type SourcedContentItem,
} from "@/api/client";
import ContentFailureAlert from "./ContentFailureAlert.vue";
import ContentGrid from "./ContentGrid.vue";
import ContentItemRow from "./ContentItemRow.vue";
import ContentPagination from "./ContentPagination.vue";
import ContentSourceBadge from "./ContentSourceBadge.vue";

const result: SourcedContentItem = {
  item: {
    resourceType: "album",
    resourceId: "virtual-album-1",
    canonicalId: "fixture:album:shared",
    title: "Virtual Album",
    subtitle: "Virtual Creator",
    description: "Deterministic local result",
  },
  source: {
    platformId: "virtual",
    pluginId: "com.audiodown.virtual.content.with-a-very-long-unbroken-id",
    pluginName: "Virtual Content",
    pluginVersion: "1.0.0",
  },
};

const failure: ContentFailure = {
  code: "RESOURCE_ACCESS_DENIED",
  summary: "Virtual catalog could not return this result",
  pluginId: "com.audiodown.catalog.content",
  platformId: "catalog",
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("typed content API", () => {
  it("preserves stable Core API error codes", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            code: "INVALID_SEARCH_REQUEST",
            message: "Search request is invalid",
          }),
          {
            status: 400,
            headers: { "content-type": "application/json" },
          },
        ),
      ),
    );

    await expect(api.search({ query: "" })).rejects.toMatchObject({
      status: 400,
      code: "INVALID_SEARCH_REQUEST",
      message: "Search request is invalid",
    });
  });
});

describe("shared content result components", () => {
  it("renders trusted source, plugin version, and long metadata safely", () => {
    const source = mount(ContentSourceBadge, {
      props: { source: result.source },
    });
    expect(source.text()).toContain("virtual");
    expect(source.text()).toContain("Virtual Content");
    expect(source.text()).toContain("1.0.0");
    expect(source.get("[data-plugin-id]").classes()).toContain("break-all");

    const row = mount(ContentItemRow, { props: { result } });
    expect(row.get('[role="button"]').attributes("tabindex")).toBe("0");
    expect(row.text()).toContain(result.item.title);
    expect(row.text()).toContain(result.item.description);
  });

  it("keeps loading geometry stable and emits result activation", async () => {
    const loading = mount(ContentGrid, {
      props: { items: [], loading: true },
    });
    expect(loading.get("[data-content-grid]").classes()).toContain("min-h-48");
    expect(loading.findAll('[data-slot="skeleton"]')).toHaveLength(3);

    const populated = mount(ContentGrid, {
      props: { items: [result], loading: false },
    });
    await populated.get('[role="button"]').trigger("keydown.enter");
    expect(populated.emitted("open")?.[0]).toEqual([result]);
  });

  it("shows safe partial failures without replacing successful content", () => {
    const wrapper = mount(ContentFailureAlert, {
      props: { failures: [failure] },
    });
    expect(wrapper.text()).toContain("部分来源暂不可用");
    expect(wrapper.text()).toContain(failure.summary);
    expect(wrapper.text()).toContain(failure.pluginId);
  });

  it("provides bounded previous and opaque next cursor controls", async () => {
    const wrapper = mount(ContentPagination, {
      props: {
        hasPrevious: true,
        nextCursor: "opaque+/=cursor",
        busy: false,
      },
    });
    await wrapper.get('button[aria-label="上一页"]').trigger("click");
    await wrapper.get('button[aria-label="下一页"]').trigger("click");
    expect(wrapper.emitted("previous")).toHaveLength(1);
    expect(wrapper.emitted("next")?.[0]).toEqual(["opaque+/=cursor"]);
  });
});
