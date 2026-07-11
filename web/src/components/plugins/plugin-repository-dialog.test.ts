import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineComponent } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import PluginRepositoryDialog from "./PluginRepositoryDialog.vue";
import ResponsiveDialog from "@/components/common/ResponsiveDialog.vue";
import PluginsView from "@/views/PluginsView.vue";

const repositoryUrl = "https://github.com/example-owner/example-repository";
const preview = {
  snapshotId: "018f0000-0000-7000-8000-000000000001",
  repository: {
    id: "example.plugins",
    name: "Example Plugins",
    sourceUrl: repositoryUrl,
    commitSha: "0123456789abcdef0123456789abcdef01234567",
  },
  plugins: [
    {
      pluginId: "com.audiodown.virtual.content",
      name: "Virtual Content",
      version: "1.0.0",
      pluginType: "content",
      alreadyInstalled: false,
      requiresLifecycleScriptGrant: false,
      lifecycleScriptReason: null,
    },
  ],
};
const installedPlugin = {
  pluginId: "com.audiodown.virtual.content",
  pluginType: "content",
  platformId: "virtual",
  name: "Virtual Content",
  version: "1.0.0",
  status: "installed",
  enabled: true,
  runMode: "on_demand",
  priority: 100,
  sourceUrl: repositoryUrl,
  commitSha: preview.repository.commitSha,
};

const ResponsiveDialogStub = defineComponent({
  name: "ResponsiveDialog",
  props: {
    open: Boolean,
    title: String,
    description: String,
  },
  emits: ["update:open", "close"],
  template: `
    <section v-if="open" data-responsive-dialog>
      <h2>{{ title }}</h2>
      <p v-if="description">{{ description }}</p>
      <slot />
      <footer><slot name="footer" /></footer>
    </section>
  `,
});

function jsonResponse(value: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => value,
  };
}

function mountDialog(
  props: Partial<{
    open: boolean;
    supervisorAvailable: boolean;
    developmentMode: boolean;
  }> = {},
) {
  return mount(PluginRepositoryDialog, {
    props: {
      open: true,
      supervisorAvailable: true,
      developmentMode: true,
      ...props,
    },
    global: {
      stubs: {
        ResponsiveDialog: ResponsiveDialogStub,
      },
    },
  });
}

async function inspect(wrapper: ReturnType<typeof mountDialog>) {
  await wrapper.get('input[name="repository-url"]').setValue(repositoryUrl);
  await wrapper.get('button[type="submit"]').trigger("click");
  await flushPromises();
}

afterEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe("plugin repository dialog", () => {
  it("uses the responsive dialog and rejects an empty URL before fetch", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const wrapper = mountDialog();

    expect(wrapper.findComponent(ResponsiveDialog).exists()).toBe(true);
    await wrapper.get('button[type="submit"]').trigger("click");

    expect(fetchMock).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("请输入公开仓库地址");
  });

  it("keeps the URL during inspection and previews repository metadata", async () => {
    let resolveInspect!: (value: unknown) => void;
    const fetchMock = vi.fn().mockReturnValue(
      new Promise((resolve) => {
        resolveInspect = resolve;
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const wrapper = mountDialog({ supervisorAvailable: false });

    await wrapper.get('input[name="repository-url"]').setValue(repositoryUrl);
    await wrapper.get('button[type="submit"]').trigger("click");
    expect(wrapper.text()).toContain("检查中");
    expect(
      (
        wrapper.get('input[name="repository-url"]')
          .element as HTMLInputElement
      ).value,
    ).toBe(repositoryUrl);

    resolveInspect(jsonResponse(preview));
    await flushPromises();

    expect(wrapper.text()).toContain("Example Plugins");
    expect(wrapper.text()).toContain("0123456");
    expect(wrapper.text()).toContain("Virtual Content");
    expect(wrapper.text()).toContain("content");
    expect(wrapper.text()).toContain("1.0.0");
    expect(wrapper.find('input[type="password"]').exists()).toBe(false);
    expect(wrapper.get('button[type="submit"]').attributes()).toHaveProperty(
      "disabled",
    );
  });

  it("sends a lifecycle token only in its header and clears it on failure", async () => {
    const riskyPreview = {
      ...preview,
      plugins: [
        {
          ...preview.plugins[0],
          requiresLifecycleScriptGrant: true,
          lifecycleScriptReason: "依赖安装阶段脚本",
        },
      ],
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(riskyPreview))
      .mockRejectedValueOnce(new Error("build failed"));
    vi.stubGlobal("fetch", fetchMock);
    const wrapper = mountDialog();
    await inspect(wrapper);

    expect(wrapper.text()).toContain("依赖安装阶段脚本");
    const checkbox = wrapper.get('[role="checkbox"]');
    expect(checkbox.attributes("aria-checked")).toBe("false");
    const token = wrapper.get(
      'input[name="audiodown-developer-token"]',
    );
    expect(token.attributes("type")).toBe("password");
    expect(token.attributes("autocomplete")).toBe("off");
    await checkbox.trigger("click");
    await token.setValue("sensitive-token");
    await wrapper.get('button[type="submit"]').trigger("click");
    await flushPromises();

    const [url, init] = fetchMock.mock.calls[1];
    expect(url).not.toContain("sensitive-token");
    expect(init.body).not.toContain("sensitive-token");
    expect(init.headers["x-audiodown-dev-token"]).toBe("sensitive-token");
    expect(JSON.parse(init.body)).toEqual({ allowLifecycleScripts: true });
    expect(wrapper.text()).toContain("安装失败");
    expect(wrapper.text()).toContain("Example Plugins");
    expect(
      (wrapper.get('input[type="password"]').element as HTMLInputElement).value,
    ).toBe("");
    expect(wrapper.text()).not.toContain("sensitive-token");
    expect(localStorage.getItem("sensitive-token")).toBeNull();
    expect(sessionStorage.getItem("sensitive-token")).toBeNull();
  });

  it("closes and emits the installed item after a normal install", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse(preview))
      .mockResolvedValueOnce(jsonResponse(installedPlugin));
    vi.stubGlobal("fetch", fetchMock);
    const wrapper = mountDialog();
    await inspect(wrapper);
    await wrapper.get('button[type="submit"]').trigger("click");
    await flushPromises();

    expect(wrapper.emitted("installed")?.[0]).toEqual([installedPlugin]);
    expect(wrapper.emitted("update:open")?.at(-1)).toEqual([false]);
    const [, installInit] = fetchMock.mock.calls[1];
    expect(installInit.headers).not.toHaveProperty(
      "x-audiodown-dev-token",
    );
  });

  it("clears token memory on every exit path without browser storage", () => {
    const source = readFileSync(
      resolve(
        process.cwd(),
        "src/components/plugins/PluginRepositoryDialog.vue",
      ),
      "utf8",
    );

    expect(source).toContain("onBeforeUnmount(clearDeveloperToken)");
    expect(source).not.toContain("localStorage");
    expect(source).not.toContain("sessionStorage");
    expect(source).not.toMatch(/toast\.[^(]+\(.*developerToken/);
  });

  it("opens the repository dialog from the plugins workspace", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve(
          jsonResponse(
            url.endsWith("/system")
              ? {
                  version: "1.0.0-alpha.1",
                  supervisor: { available: true, error: null },
                  pluginCount: 0,
                  developmentMode: true,
                }
              : { items: [] },
          ),
        ),
      ),
    );
    const wrapper = mount(PluginsView, {
      global: {
        stubs: {
          ResponsiveDialog: ResponsiveDialogStub,
        },
      },
    });
    await flushPromises();

    const dialog = wrapper.findComponent(PluginRepositoryDialog);
    expect(dialog.props("open")).toBe(false);
    await wrapper.findAll("button")[0].trigger("click");
    expect(dialog.props("open")).toBe(true);
    expect(dialog.props("supervisorAvailable")).toBe(true);
    expect(dialog.props("developmentMode")).toBe(true);
  });
});
