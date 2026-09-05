use ash::vk;
use gtk::prelude::*;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

// -----------------------------------------------------------------------------
// VULKAN LOADER FFI STRUCTS
// -----------------------------------------------------------------------------
const VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO: vk::StructureType =
    vk::StructureType::from_raw(47);
const VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO: vk::StructureType =
    vk::StructureType::from_raw(48);

#[repr(C)]
enum VkLayerFunction {
    LinkInfo = 0,
    AllocateCallbacks = 1,
    SetDeviceLoaderData = 2,
}

#[repr(C)]
struct VkLayerInstanceLink {
    p_next: *mut VkLayerInstanceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

#[repr(C)]
struct VkLayerDeviceLink {
    p_next: *mut VkLayerDeviceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

#[repr(C)]
struct VkLayerInstanceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: VkLayerFunction,
    p_layer_info: *mut VkLayerInstanceLink,
}

#[repr(C)]
struct VkLayerDeviceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: VkLayerFunction,
    p_layer_info: *mut VkLayerDeviceLink,
}

// -----------------------------------------------------------------------------
// GLOBAL DISPATCH TABLES & STATE
// -----------------------------------------------------------------------------
struct InstanceDispatch {
    get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    enumerate_physical_devices: vk::PFN_vkEnumeratePhysicalDevices,
    enumerate_physical_device_groups: Option<vk::PFN_vkEnumeratePhysicalDeviceGroups>,
    enumerate_physical_device_groups_khr: Option<vk::PFN_vkEnumeratePhysicalDeviceGroups>,
    get_physical_device_properties: vk::PFN_vkGetPhysicalDeviceProperties,
    destroy_instance: vk::PFN_vkDestroyInstance,
}

struct DeviceDispatch {
    get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    destroy_device: vk::PFN_vkDestroyDevice,
}

lazy_static! {
    static ref INSTANCE_DISPATCH: Mutex<HashMap<vk::Instance, InstanceDispatch>> =
        Mutex::new(HashMap::new());
    static ref DEVICE_DISPATCH: Mutex<HashMap<vk::Device, DeviceDispatch>> =
        Mutex::new(HashMap::new());
}

static CACHED_DEVICE_INDEX: AtomicUsize = AtomicUsize::new(9999);
const ENV_VARIABLE: &str = "VULKAN_DEVICE_INDEX";

// -----------------------------------------------------------------------------
// GTK & FILE HELPERS
// -----------------------------------------------------------------------------
fn run_gtk_pick(options: &[String]) -> i32 {
    gtk::init().expect("Failed to initialize GTK.");

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Vulkan Device Chooser");
    window.set_border_width(15);
    window.set_default_size(400, 100);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    window.add(&vbox);

    let label = gtk::Label::new(Some("Select the GPU you want to use for this application:"));
    vbox.pack_start(&label, false, false, 0);

    let combo_box = gtk::ComboBoxText::new();
    for (i, opt) in options.iter().enumerate() {
        combo_box.append(Some(&i.to_string()), opt);
    }
    combo_box.set_active(Some(0));
    vbox.pack_start(&combo_box, true, true, 0);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    vbox.pack_start(&hbox, false, false, 0);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hbox.pack_start(&spacer, true, true, 0);

    let button = gtk::Button::with_label("Apply");
    hbox.pack_start(&button, false, false, 0);

    use std::cell::RefCell;
    use std::rc::Rc;
    let result = Rc::new(RefCell::new(0));
    let result_clone = result.clone();

    combo_box.connect_changed({
        let result_clone = result_clone.clone();
        move |combo| {
            if let Some(idx) = combo.active() {
                *result_clone.borrow_mut() = idx as i32;
            }
        }
    });

    let window_clone = window.clone();
    button.connect_clicked(move |_| {
        window_clone.close();
    });

    window.connect_destroy(|_| {
        gtk::main_quit();
    });

    window.show_all();
    gtk::main();

    let final_index = *result.borrow();
    final_index
}

fn get_family_index(family_name: &str) -> Option<i32> {
    let path = format!("/tmp/vkdevicechooser/{}", family_name);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn set_family_index(index: i32, family_name: &str) {
    let dir = "/tmp/vkdevicechooser";
    let _ = std::fs::create_dir_all(dir);
    let path = format!("{}/{}", dir, family_name);
    let _ = std::fs::write(path, index.to_string());
}

// -----------------------------------------------------------------------------
// DEVICE CHOOSING LOGIC
// -----------------------------------------------------------------------------
unsafe fn choose_device(
    instance: vk::Instance,
    dispatch: &InstanceDispatch,
    env: &str,
    out_device: &mut vk::PhysicalDevice,
) -> vk::Result {
    let mut count = 0;
    let mut res = (dispatch.enumerate_physical_devices)(instance, &mut count, std::ptr::null_mut());

    if res != vk::Result::SUCCESS {
        return res;
    }
    if count == 0 {
        *out_device = vk::PhysicalDevice::null();
        return vk::Result::SUCCESS;
    }

    let mut devices = vec![vk::PhysicalDevice::null(); count as usize];
    res = (dispatch.enumerate_physical_devices)(instance, &mut count, devices.as_mut_ptr());

    if res != vk::Result::SUCCESS {
        return res;
    }

    // Fast path across temporary WSI instances
    let current_cached = CACHED_DEVICE_INDEX.load(Ordering::Relaxed);
    if current_cached < 9999 {
        *out_device = devices[current_cached];
        return vk::Result::SUCCESS;
    }

    println!("Found {} devices", count);

    if let Ok(idx) = env.parse::<usize>() {
        let mut chosen = idx;
        if chosen >= count as usize {
            eprintln!("Device index {} does not exist, returning device 0", chosen);
            chosen = 0;
        } else {
            println!("Using Vulkan device index {}", chosen);
        }
        CACHED_DEVICE_INDEX.store(chosen, Ordering::Relaxed);
        *out_device = devices[chosen];
        return vk::Result::SUCCESS;
    }

    if env.starts_with("name:") {
        let gpu_name = &env[5..];
        let mut chosen = 9999;
        for (i, &device) in devices.iter().enumerate() {
            let mut props = vk::PhysicalDeviceProperties::default();
            (dispatch.get_physical_device_properties)(device, &mut props);

            let name_cstr = CStr::from_ptr(props.device_name.as_ptr());
            let name_str = name_cstr.to_string_lossy();

            println!("Device {}: ({}) {}", i, props.device_id, name_str);
            if name_str.contains(gpu_name) {
                chosen = i;
            }
        }

        if chosen >= count as usize {
            eprintln!("Device does not exist, returning device 0");
            chosen = 0;
        } else {
            println!("Using Vulkan device index {}", chosen);
        }

        CACHED_DEVICE_INDEX.store(chosen, Ordering::Relaxed);
        *out_device = devices[chosen];
        return vk::Result::SUCCESS;
    }

    if env == "letmechoose" || env.starts_with("letmechoose:") {
        let mut device_names = Vec::new();
        for &device in &devices {
            let mut props = vk::PhysicalDeviceProperties::default();
            (dispatch.get_physical_device_properties)(device, &mut props);
            let name_cstr = CStr::from_ptr(props.device_name.as_ptr());
            device_names.push(name_cstr.to_string_lossy().into_owned());
        }

        let chosen: usize;
        if let Some(family) = env.strip_prefix("letmechoose:") {
            if let Some(cached_idx) = get_family_index(family) {
                chosen = cached_idx as usize;
            } else {
                chosen = run_gtk_pick(&device_names) as usize;
                set_family_index(chosen as i32, family);
            }
        } else {
            chosen = run_gtk_pick(&device_names) as usize;
        }

        println!(
            "Using Vulkan device \"{}\" with index {}",
            device_names[chosen], chosen
        );
        CACHED_DEVICE_INDEX.store(chosen, Ordering::Relaxed);
        *out_device = devices[chosen];
        return vk::Result::SUCCESS;
    }

    eprintln!("Bad usage of vkdevicechooser, returning device 0");
    CACHED_DEVICE_INDEX.store(0, Ordering::Relaxed);
    *out_device = devices[0];
    vk::Result::SUCCESS
}

// -----------------------------------------------------------------------------
// EXPORTED VULKAN LAYER FUNCTIONS
// -----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_EnumeratePhysicalDevices(
    instance: vk::Instance,
    p_physical_device_count: *mut u32,
    p_physical_devices: *mut vk::PhysicalDevice,
) -> vk::Result {
    let map = INSTANCE_DISPATCH.lock().unwrap();
    let dispatch = map.get(&instance).expect("Instance not found in dispatch");

    let env = std::env::var(ENV_VARIABLE).unwrap_or_default();
    if env.is_empty() {
        return (dispatch.enumerate_physical_devices)(
            instance,
            p_physical_device_count,
            p_physical_devices,
        );
    }

    let mut device = vk::PhysicalDevice::null();
    let res = choose_device(instance, dispatch, &env, &mut device);

    if res != vk::Result::SUCCESS {
        return res;
    }

    if device == vk::PhysicalDevice::null() {
        *p_physical_device_count = 0;
    } else if p_physical_devices.is_null() {
        *p_physical_device_count = 1;
    } else if *p_physical_device_count > 0 {
        *p_physical_devices = device;
        *p_physical_device_count = 1;
    }

    vk::Result::SUCCESS
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_EnumeratePhysicalDeviceGroups(
    instance: vk::Instance,
    p_physical_device_group_count: *mut u32,
    p_physical_device_group_properties: *mut vk::PhysicalDeviceGroupProperties,
) -> vk::Result {
    let map = INSTANCE_DISPATCH.lock().unwrap();
    let dispatch = map.get(&instance).expect("Instance not found in dispatch");

    let enum_groups = match dispatch
        .enumerate_physical_device_groups
        .or(dispatch.enumerate_physical_device_groups_khr)
    {
        Some(f) => f,
        None => {
            *p_physical_device_group_count = 0;
            return vk::Result::SUCCESS;
        }
    };

    let env = std::env::var(ENV_VARIABLE).unwrap_or_default();
    if env.is_empty() {
        return (enum_groups)(
            instance,
            p_physical_device_group_count,
            p_physical_device_group_properties,
        );
    }

    let mut device = vk::PhysicalDevice::null();
    let res = choose_device(instance, dispatch, &env, &mut device);
    if res != vk::Result::SUCCESS {
        return res;
    }

    if device == vk::PhysicalDevice::null() {
        *p_physical_device_group_count = 0;
        return vk::Result::SUCCESS;
    }

    if p_physical_device_group_properties.is_null() {
        *p_physical_device_group_count = 1;
        return vk::Result::SUCCESS;
    }

    if *p_physical_device_group_count > 0 {
        let mut real_count = 0;
        (enum_groups)(instance, &mut real_count, std::ptr::null_mut());

        let mut real_groups =
            vec![vk::PhysicalDeviceGroupProperties::default(); real_count as usize];
        (enum_groups)(instance, &mut real_count, real_groups.as_mut_ptr());

        for mut group in real_groups {
            let count = group.physical_device_count as usize;
            let devices = &group.physical_devices[..count];
            if devices.contains(&device) {
                group.physical_device_count = 1;
                let mut fake_devices = [vk::PhysicalDevice::null(); 32];
                fake_devices[0] = device;
                group.physical_devices = fake_devices;

                *p_physical_device_group_properties = group;
                *p_physical_device_group_count = 1;
                return vk::Result::SUCCESS;
            }
        }
        *p_physical_device_group_count = 0;
    }

    vk::Result::SUCCESS
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_EnumeratePhysicalDeviceGroupsKHR(
    instance: vk::Instance,
    p_physical_device_group_count: *mut u32,
    p_physical_device_group_properties: *mut vk::PhysicalDeviceGroupProperties,
) -> vk::Result {
    DeviceChooserLayer_EnumeratePhysicalDeviceGroups(
        instance,
        p_physical_device_group_count,
        p_physical_device_group_properties,
    )
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_CreateInstance(
    p_create_info: *const vk::InstanceCreateInfo,
    p_allocator: *const vk::AllocationCallbacks,
    p_instance: *mut vk::Instance,
) -> vk::Result {
    let mut layer_info = (*p_create_info).p_next as *mut VkLayerInstanceCreateInfo;

    while !layer_info.is_null() {
        if (*layer_info).s_type == VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO
            && matches!((*layer_info).function, VkLayerFunction::LinkInfo)
        {
            break;
        }
        layer_info = (*layer_info).p_next as *mut VkLayerInstanceCreateInfo;
    }

    if layer_info.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }

    let gpa = (*(*layer_info).p_layer_info).pfn_next_get_instance_proc_addr;
    (*layer_info).p_layer_info = (*(*layer_info).p_layer_info).p_next;

    let create_instance_name = CString::new("vkCreateInstance").unwrap();
    let create_func_ptr = gpa(vk::Instance::null(), create_instance_name.as_ptr())
        .expect("Failed to get vkCreateInstance");
    let create_func: vk::PFN_vkCreateInstance = std::mem::transmute(create_func_ptr);

    let res = create_func(p_create_info, p_allocator, p_instance);
    if res != vk::Result::SUCCESS {
        return res;
    }

    let instance = *p_instance;
    let get_proc = |name: &str| -> Option<unsafe extern "system" fn()> {
        let cname = CString::new(name).unwrap();
        gpa(instance, cname.as_ptr())
    };

    let dispatch_table = InstanceDispatch {
        get_instance_proc_addr: gpa,
        destroy_instance: std::mem::transmute(get_proc("vkDestroyInstance").unwrap()),
        enumerate_physical_devices: std::mem::transmute(
            get_proc("vkEnumeratePhysicalDevices").unwrap(),
        ),
        enumerate_physical_device_groups: get_proc("vkEnumeratePhysicalDeviceGroups")
            .map(|f| std::mem::transmute(f)),
        enumerate_physical_device_groups_khr: get_proc("vkEnumeratePhysicalDeviceGroupsKHR")
            .map(|f| std::mem::transmute(f)),
        get_physical_device_properties: std::mem::transmute(
            get_proc("vkGetPhysicalDeviceProperties").unwrap(),
        ),
    };

    INSTANCE_DISPATCH
        .lock()
        .unwrap()
        .insert(instance, dispatch_table);
    vk::Result::SUCCESS
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_DestroyInstance(
    instance: vk::Instance,
    p_allocator: *const vk::AllocationCallbacks,
) {
    let mut map = INSTANCE_DISPATCH.lock().unwrap();
    if let Some(dispatch) = map.remove(&instance) {
        (dispatch.destroy_instance)(instance, p_allocator);
    }
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_CreateDevice(
    physical_device: vk::PhysicalDevice,
    p_create_info: *const vk::DeviceCreateInfo,
    p_allocator: *const vk::AllocationCallbacks,
    p_device: *mut vk::Device,
) -> vk::Result {
    let mut layer_info = (*p_create_info).p_next as *mut VkLayerDeviceCreateInfo;

    while !layer_info.is_null() {
        if (*layer_info).s_type == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO
            && matches!((*layer_info).function, VkLayerFunction::LinkInfo)
        {
            break;
        }
        layer_info = (*layer_info).p_next as *mut VkLayerDeviceCreateInfo;
    }

    if layer_info.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }

    let gipa = (*(*layer_info).p_layer_info).pfn_next_get_instance_proc_addr;
    let gdpa = (*(*layer_info).p_layer_info).pfn_next_get_device_proc_addr;
    (*layer_info).p_layer_info = (*(*layer_info).p_layer_info).p_next;

    let create_device_name = CString::new("vkCreateDevice").unwrap();
    let create_func_ptr = gipa(vk::Instance::null(), create_device_name.as_ptr())
        .expect("Failed to get vkCreateDevice");
    let create_func: vk::PFN_vkCreateDevice = std::mem::transmute(create_func_ptr);

    let res = create_func(physical_device, p_create_info, p_allocator, p_device);
    if res != vk::Result::SUCCESS {
        return res;
    }

    let device = *p_device;
    let destroy_name = CString::new("vkDestroyDevice").unwrap();
    let destroy_ptr = gdpa(device, destroy_name.as_ptr()).unwrap();

    let dispatch_table = DeviceDispatch {
        get_device_proc_addr: gdpa,
        destroy_device: std::mem::transmute(destroy_ptr),
    };

    DEVICE_DISPATCH
        .lock()
        .unwrap()
        .insert(device, dispatch_table);
    vk::Result::SUCCESS
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_DestroyDevice(
    device: vk::Device,
    p_allocator: *const vk::AllocationCallbacks,
) {
    let mut map = DEVICE_DISPATCH.lock().unwrap();
    if let Some(dispatch) = map.remove(&device) {
        (dispatch.destroy_device)(device, p_allocator);
    }
}

// -----------------------------------------------------------------------------
// GET PROC ADDR HOOKS
// -----------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_GetDeviceProcAddr(
    device: vk::Device,
    p_name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let name = CStr::from_ptr(p_name).to_str().unwrap_or("");

    // CRITICAL FIX: We must NOT intercept vkCreateDevice here!
    if name == "vkGetDeviceProcAddr" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_GetDeviceProcAddr as *const (),
        ));
    }
    if name == "vkDestroyDevice" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_DestroyDevice as *const (),
        ));
    }

    let map = DEVICE_DISPATCH.lock().unwrap();
    if let Some(dispatch) = map.get(&device) {
        return (dispatch.get_device_proc_addr)(device, p_name);
    }
    None
}

#[no_mangle]
pub unsafe extern "system" fn DeviceChooserLayer_GetInstanceProcAddr(
    instance: vk::Instance,
    p_name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    let name = CStr::from_ptr(p_name).to_str().unwrap_or("");

    if name == "vkGetInstanceProcAddr" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_GetInstanceProcAddr as *const (),
        ));
    }
    if name == "vkCreateInstance" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_CreateInstance as *const (),
        ));
    }
    if name == "vkDestroyInstance" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_DestroyInstance as *const (),
        ));
    }
    if name == "vkEnumeratePhysicalDevices" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_EnumeratePhysicalDevices as *const (),
        ));
    }
    if name == "vkEnumeratePhysicalDeviceGroups" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_EnumeratePhysicalDeviceGroups as *const (),
        ));
    }
    if name == "vkEnumeratePhysicalDeviceGroupsKHR" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_EnumeratePhysicalDeviceGroupsKHR as *const (),
        ));
    }
    if name == "vkCreateDevice" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_CreateDevice as *const (),
        ));
    }

    // Pass-through explicitly hooked device commands if queried via Instance
    if name == "vkGetDeviceProcAddr" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_GetDeviceProcAddr as *const (),
        ));
    }
    if name == "vkDestroyDevice" {
        return Some(std::mem::transmute(
            DeviceChooserLayer_DestroyDevice as *const (),
        ));
    }

    let map = INSTANCE_DISPATCH.lock().unwrap();
    if let Some(dispatch) = map.get(&instance) {
        return (dispatch.get_instance_proc_addr)(instance, p_name);
    }
    None
}
