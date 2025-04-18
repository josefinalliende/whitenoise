<script lang="ts">
import { activeAccount } from "$lib/stores/accounts";
import { getToastState } from "$lib/stores/toast-state.svelte";
import type { CachedMessage, Message } from "$lib/types/chat";
import type { NostrMlsGroup, NostrMlsGroupWithRelays } from "$lib/types/nostr";
import { hexMlsGroupId } from "$lib/utils/group";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { readFile } from "@tauri-apps/plugin-fs";
import Check from "phosphor-svelte/lib/Check";
import PaperPlaneTilt from "phosphor-svelte/lib/PaperPlaneTilt";
import Plus from "phosphor-svelte/lib/Plus";
import TrashSimple from "phosphor-svelte/lib/TrashSimple";
import X from "phosphor-svelte/lib/X";
import { onMount } from "svelte";
import Loader from "./Loader.svelte";

let {
    group,
    replyToMessage = $bindable(),
    handleNewMessage,
    isReplyToMessageDeleted = $bindable(false),
}: {
    group: NostrMlsGroup;
    replyToMessage?: Message;
    handleNewMessage: (message: CachedMessage) => void;
    isReplyToMessageDeleted?: boolean;
} = $props();

let message = $state("");
let media = $state<
    Array<{
        file: File;
        status: "uploading" | "error" | "success";
    }>
>([]);
let textarea: HTMLTextAreaElement;
let sendingMessage: boolean = $state(false);
let toastState = getToastState();

$inspect(media);

function adjustTextareaHeight() {
    textarea.style.height = "auto";
    textarea.style.height = `${textarea.scrollHeight}px`;
}

function handleInput() {
    adjustTextareaHeight();
}

async function sendMessage() {
    if (message.length === 0 && media.length === 0) return;

    // Check if any uploads are still in progress
    if (media.some((item) => item.status === "uploading")) {
        toastState.add("Uploads in progress", "Please wait for uploads to complete", "info");
        return;
    }

    let kind = 9;
    let tags = [];
    if (replyToMessage) {
        let groupWithRelays: NostrMlsGroupWithRelays = await invoke("get_group", {
            groupId: hexMlsGroupId(group.mls_group_id),
        });
        tags.push(["q", replyToMessage.id, groupWithRelays.relays[0], replyToMessage.pubkey]);
    }

    let tmpMessage = {
        id: "temp",
        content: message,
        created_at: Math.floor(Date.now() / 1000),
        pubkey: $activeAccount?.pubkey,
        kind,
        tags,
    };

    handleNewMessage(tmpMessage as unknown as CachedMessage);
    sendingMessage = true;

    await invoke("send_mls_message", {
        group,
        message,
        kind,
        tags,
        uploadedFiles: await Promise.all(
            media
                .filter((item) => item.status === "success")
                .map(async (item) => {
                    const arrayBuffer = await item.file.arrayBuffer();
                    return {
                        filename: item.file.name,
                        mime_type: item.file.type,
                        data: Array.from(new Uint8Array(arrayBuffer)),
                    };
                })
        ),
    })
        .then((cachedMessage) => {
            handleNewMessage(cachedMessage as CachedMessage);
            message = "";
            media = []; // Clear media after successful send
            setTimeout(adjustTextareaHeight, 0);
        })
        .finally(() => {
            replyToMessage = undefined;
            sendingMessage = false;
        });
}

function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Enter") {
        sendMessage();
    }
}

async function handleFileUpload() {
    const filePath = await open({
        multiple: false,
        directory: false,
        mimeTypes: ["image/*", "video/*", "audio/*", "application/pdf"],
    });
    if (!filePath) return;

    try {
        const fileData = await readFile(filePath);
        // Create a File object from the array buffer
        const file = new File([fileData], filePath.split("/").pop() || "file", {
            type: getMimeType(filePath),
        });

        // Add file to media array and start upload
        media = [...media, { file, status: "uploading" }];
        await uploadFile(file);
    } catch (error) {
        console.error("Error reading file:", error);
        toastState.add("Error", "Failed to read file", "error");
    }
}

async function uploadFile(file: File) {
    try {
        const arrayBuffer = await file.arrayBuffer();
        const fileData = {
            filename: file.name,
            mime_type: file.type,
            data: Array.from(new Uint8Array(arrayBuffer)),
        };

        await invoke("upload_file", {
            groupId: group.mls_group_id,
            file: fileData,
        });

        // Update status to success
        media = media.map((item) =>
            item.file.name === file.name ? { ...item, status: "success" } : item
        );
    } catch (error) {
        console.error("Error uploading file:", error);
        media = media.map((item) =>
            item.file.name === file.name ? { ...item, status: "error" } : item
        );
        toastState.add("Error", `Failed to upload ${file.name}`, "error");
    }
}

// Helper function to determine MIME type from file extension
function getMimeType(filePath: string): string {
    const extension = filePath.split(".").pop()?.toLowerCase();
    const mimeTypes: Record<string, string> = {
        jpg: "image/jpeg",
        jpeg: "image/jpeg",
        png: "image/png",
        gif: "image/gif",
        mp4: "video/mp4",
        mp3: "audio/mpeg",
        pdf: "application/pdf",
        // Add more as needed
    };
    return mimeTypes[extension || ""] || "application/octet-stream";
}

onMount(() => {
    const visualViewport = window.visualViewport;
    if (visualViewport) {
        const onResize = () => {
            const isKeyboardVisible = visualViewport.height < window.innerHeight;
            document.body.classList.toggle("keyboard-visible", isKeyboardVisible);
        };
        visualViewport.addEventListener("resize", onResize);
        return () => visualViewport.removeEventListener("resize", onResize);
    }
});
</script>

<div class="messagebar sticky bottom-0 left-0 right-0 bg-gray-900 drop-shadow-message-bar">
    {#if replyToMessage}
        <div class="w-full py-4 px-6 pl-8 bg-blue-700/50 text-white backdrop-blur-sm border-t border-gray-700 border-l-4 border-l-blue-500 flex flex-row gap-2 items-start justify-between rounded-t-xl">
            {#if isReplyToMessageDeleted}
                <div class="inline-flex flex-row items-center gap-2 bg-gray-200 rounded-full px-3 py-1 w-fit text-black">
                    <TrashSimple size={20} /><span class="italic opacity-60">Message deleted</span>
                </div>
            {:else}
                <span>{replyToMessage.content}</span>
            {/if}
            <button onclick={() => replyToMessage = undefined} class="p-1 bg-white/50 hover:bg-white rounded-full mr-0">
                <X size={12} class="text-blue-700" />
            </button>
        </div>
    {/if}
    {#if media.length > 0}
        <div class="w-full py-2 px-6 pl-8 bg-gray-800/50 backdrop-blur-sm border-t border-gray-700 flex flex-row gap-2 items-center overflow-x-auto">
            {#each media as item, index}
                <div class="relative group">
                    {#if item.file.type.startsWith('image/')}
                        <img
                            src={URL.createObjectURL(item.file)}
                            alt="Preview"
                            class="h-16 w-16 object-cover rounded-lg"
                        />
                    {:else if item.file.type.startsWith('video/')}
                        <div class="h-16 w-16 bg-gray-700 rounded-lg flex items-center justify-center">
                            <span class="text-white text-sm">Video</span>
                        </div>
                    {:else if item.file.type.startsWith('audio/')}
                        <div class="h-16 w-16 bg-gray-700 rounded-lg flex items-center justify-center">
                            <span class="text-white text-sm">Audio</span>
                        </div>
                    {:else}
                        <div class="h-16 w-16 bg-gray-700 rounded-lg flex items-center justify-center">
                            <span class="text-white text-sm">PDF</span>
                        </div>
                    {/if}
                    <div class="absolute inset-0 bg-black/50 rounded-lg flex items-center justify-center">
                        {#if item.status === 'uploading'}
                            <div class="w-12 h-12">
                                <Loader fullscreen={false} size={48} />
                            </div>
                        {:else if item.status === 'error'}
                            <div class="text-red-500">
                                <X size={24} />
                            </div>
                        {:else if item.status === 'success'}
                            <div class="text-green-500">
                                <Check size={24} />
                            </div>
                        {/if}
                    </div>
                    <button
                        class="absolute -top-2 -right-2 bg-red-500 text-white rounded-full p-1 opacity-0 group-hover:opacity-100 transition-opacity"
                        onclick={() => {
                            media = media.filter((_, i) => i !== index);
                        }}
                    >
                        <X size={12} />
                    </button>
                </div>
            {/each}
        </div>
    {/if}
    <div class="flex flex-row px-8 py-4 gap-4 items-center border-t border-gray-700">
        <textarea
            id="newMessageInput"
            bind:this={textarea}
            class="px-4 py-2 w-full bg-transparent ring-1 ring-gray-700 rounded-lg min-h-[2.5rem] max-h-[12rem] resize-none overflow-y-auto"
            rows="1"
            bind:value={message}
            oninput={handleInput}
            onkeydown={handleKeydown}
        ></textarea>
        <div class="flex flex-row gap-2">
            {#if message.length > 0}
                <button
                    class="p-2 bg-blue-700 rounded-full text-white ring-1 ring-blue-500 hover:bg-blue-600 disabled:hidden"
                onclick={sendMessage}
                disabled={sendingMessage || media.some(item => item.status === "uploading")}
            >
                <PaperPlaneTilt size={24} weight="regular" class="" />
            </button>
        {:else}
            <button
                class="p-2 rounded-full text-white ring-1 ring-gray-900 rounded-full text-white disabled:hidden"
                onclick={handleFileUpload}
                disabled={false}
            >
                <Plus size={24} weight="light" class="" />
            </button>
            {/if}
        </div>


        <div
            class="p-3 bg-blue-700 rounded-full text-white ring-1 ring-blue-500"
            class:hidden={!sendingMessage}
        >
            <Loader fullscreen={false} size={24} />
        </div>
    </div>
</div>

<style>
    :global(body.keyboard-visible) .messagebar {
        position: fixed;
        bottom: 0;
        width: 100%;
    }
</style>
