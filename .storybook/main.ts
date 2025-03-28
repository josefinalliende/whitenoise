import type { StorybookConfig } from "@storybook/sveltekit";

const config: StorybookConfig = {
    stories: ["../src/**/*.stories.@(ts|svelte)"],
    addons: ["@storybook/addon-essentials", "@storybook/addon-svelte-csf"],
    framework: {
        name: "@storybook/sveltekit",
        options: {},
    },
    core: {
        builder: '@storybook/builder-vite',
    },
    viteFinal: async (config) => {
        if (config.build) {
            config.build.chunkSizeWarningLimit = 1000;
            config.build.rollupOptions = {
                output: {
                    manualChunks: {
                        vendor: ['@storybook/sveltekit', '@storybook/addon-essentials'],
                    },
                },
            };
        }
        return config;
    }
};

export default config;
