import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  api,
  type AlbumResponse,
  type TracksResponse,
} from "@/api/client";
import AlbumView from "./AlbumView.vue";

const pluginId = "com.audiodown.virtual.content";
const resourceId = "opaque/album?identity=1";
const source = {
  platformId: "virtual",
  pluginId,
  pluginName: "Virtual Content",
  pluginVersion: "1.0.0",
};

const album: AlbumResponse = {
  album: {
    resourceId,
    canonicalId: "fixture:album:shared",
    title: "Virtual Primary Album",
    creator: "Virtual Primary Creator",
    description:
      "Deterministic local album with verylongunbrokenmetadatavalueforresponsivechecks",
    trackCount: 2,
  },
  source,
};

function tracks(
  overrides: Partial<TracksResponse> = {},
): TracksResponse {
  return {
    items: [
      {
        resourceId: "virtual-track-1",
        canonicalId: "fixture:track:1",
        title: "Virtual Track 1",
        sequence: 1,
        durationSeconds: 60,
      },
    ],
    source,
    nextCursor: "opaque-tracks-page-2",
    ...overrides,
  };
}

async function mountView(query: Record<string, string> = {
  pluginId,
  resourceId,
}) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/discover", component: { template: "<div>Discover</div>" } },
      {
        path: "/albums/detail",
        name: "album",
        component: AlbumView,
      },
    ],
  });
  await router.push({ name: "album", query });
  await router.isReady();
  return {
    router,
    wrapper: mount(AlbumView, {
      global: { plugins: [router] },
    }),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("album and tracks workspace", () => {
  it("loads source-bound album metadata and the first track page", async () => {
    vi.spyOn(api, "album").mockResolvedValue(album);
    vi.spyOn(api, "tracks").mockResolvedValue(tracks());

    const { wrapper } = await mountView();
    await flushPromises();

    expect(api.album).toHaveBeenCalledWith(pluginId, resourceId);
    expect(api.tracks).toHaveBeenCalledWith(
      pluginId,
      resourceId,
      undefined,
    );
    expect(wrapper.text()).toContain("Virtual Primary Album");
    expect(wrapper.text()).toContain("Virtual Primary Creator");
    expect(wrapper.text()).toContain("Virtual Content");
    expect(wrapper.text()).toContain("1.0.0");
    expect(wrapper.text()).toContain("Virtual Track 1");
    expect(wrapper.text()).toContain("1:00");
    expect(wrapper.get('[data-track-id="virtual-track-1"]').element.tagName)
      .toBe("LI");
    expect(
      wrapper.get('button[aria-label="下一页"]').attributes("aria-label"),
    ).toBe("下一页");
  });

  it("keeps opaque track cursors and album identity across pagination", async () => {
    vi.spyOn(api, "album").mockResolvedValue(album);
    vi.spyOn(api, "tracks")
      .mockResolvedValueOnce(tracks())
      .mockResolvedValueOnce(
        tracks({
          items: [
            {
              resourceId: "virtual-track-2",
              canonicalId: "fixture:track:2",
              title: "Virtual Track 2",
              sequence: 2,
              durationSeconds: 125,
            },
          ],
          nextCursor: null,
        }),
      )
      .mockResolvedValueOnce(tracks());

    const { wrapper } = await mountView();
    await flushPromises();
    await wrapper.get('button[aria-label="下一页"]').trigger("click");
    await flushPromises();

    expect(api.tracks).toHaveBeenLastCalledWith(
      pluginId,
      resourceId,
      "opaque-tracks-page-2",
    );
    expect(wrapper.text()).toContain("Virtual Track 2");
    expect(wrapper.text()).toContain("2:05");

    await wrapper.get('button[aria-label="上一页"]').trigger("click");
    await flushPromises();
    expect(api.tracks).toHaveBeenLastCalledWith(
      pluginId,
      resourceId,
      undefined,
    );
  });

  it("rejects incomplete source links without calling Core", async () => {
    const albumRequest = vi.spyOn(api, "album");
    const tracksRequest = vi.spyOn(api, "tracks");
    const { wrapper } = await mountView({ pluginId });
    await flushPromises();

    expect(albumRequest).not.toHaveBeenCalled();
    expect(tracksRequest).not.toHaveBeenCalled();
    expect(wrapper.get('[role="alert"]').text()).toContain("专辑链接无效");
  });

  it("retries the failed track page with the same opaque cursor", async () => {
    vi.spyOn(api, "album").mockResolvedValue(album);
    vi.spyOn(api, "tracks")
      .mockResolvedValueOnce(tracks())
      .mockRejectedValueOnce(
        new ApiError(503, "PLUGIN_UNAVAILABLE", "raw-plugin-secret"),
      )
      .mockResolvedValueOnce(
        tracks({
          items: [
            {
              resourceId: "virtual-track-2",
              title: "Virtual Track 2",
              sequence: 2,
              durationSeconds: 125,
            },
          ],
          nextCursor: null,
        }),
      );

    const { wrapper } = await mountView();
    await flushPromises();
    await wrapper.get('button[aria-label="下一页"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain(
      "PLUGIN_UNAVAILABLE",
    );

    await wrapper.get('button[aria-label="重试曲目"]').trigger("click");
    await flushPromises();
    expect(api.tracks).toHaveBeenLastCalledWith(
      pluginId,
      resourceId,
      "opaque-tracks-page-2",
    );
    expect(wrapper.text()).toContain("Virtual Track 2");
  });

  it("shows a safe not-found error and supports retry", async () => {
    vi.spyOn(api, "album")
      .mockRejectedValueOnce(
        new ApiError(404, "RESOURCE_NOT_FOUND", "raw-plugin-secret"),
      )
      .mockResolvedValueOnce(album);
    vi.spyOn(api, "tracks").mockResolvedValue(tracks());

    const { wrapper } = await mountView();
    await flushPromises();

    expect(wrapper.get('[role="alert"]').text()).toContain(
      "RESOURCE_NOT_FOUND",
    );
    expect(wrapper.text()).not.toContain("raw-plugin-secret");
    await wrapper.get('button[aria-label="重试"]').trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("Virtual Primary Album");
    expect(api.album).toHaveBeenCalledTimes(2);
  });
});
