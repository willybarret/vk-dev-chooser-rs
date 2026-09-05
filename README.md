# Vulkan Device Chooser Layer

This is a Vulkan layer that forces a specific physical device to be used. This is useful for Vulkan games which do not provide an option to choose the device themselves.

---

**Compiling**

Compiling should only require the Rust toolchain. Build with:

```bash
cargo build --release

```

**Installation**

Copy the compiled library to the path referenced by `library_path` in `vk_dev_chooser.json`, and copy the manifest itself into the system's Vulkan layer directory:

```bash
sudo cp target/release/libvk_dev_chooser.so /usr/lib64/libvk_dev_chooser.so
sudo cp vk_dev_chooser.json /usr/share/vulkan/implicit_layer.d/

```

---

**Usage**

To run a Vulkan application forcing a specific device to be used, launch it with these environment variables:

```bash
ENABLE_DEVICE_CHOOSER_LAYER=1 VULKAN_DEVICE_INDEX=<device index>

```

Replace `<device index>` with the "GPU id" for the desired device as reported by `vulkaninfo` (without the layer enabled).

For example:

```bash
$ ENABLE_DEVICE_CHOOSER_LAYER=1 VULKAN_DEVICE_INDEX=1 vulkaninfo

```

should give info for the device which had GPU id 1 when running `vulkaninfo` without the environment variable set.

**Additional Selection Options**

`VULKAN_DEVICE_INDEX` also accepts:

* `name:<substring>` to match a device by name instead of index
* `letmechoose` to pick the device from a popup window at launch (relies on `zenity` or `kdialog`, which can be chosen with `VULKAN_DEVICE_CHOOSER_GUI`)
* `letmechoose:<family>` to do the same but remember the choice for that family afterward

---

**Steam Launch Options**

The layer can be used with Steam games by setting their launch options to:

```bash
ENABLE_DEVICE_CHOOSER_LAYER=1 VULKAN_DEVICE_INDEX=<device index> %command%

```

## Credits

Rewritten in Rust, with inspiration from [vkdevicechooser](https://github.com/aejsmith/vkdevicechooser/).