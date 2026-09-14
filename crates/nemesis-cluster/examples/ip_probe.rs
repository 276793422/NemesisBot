use nemesis_cluster::network::{get_all_local_ips, get_interface_priority, is_virtual_interface};
use if_addrs::get_if_addrs;

fn main() {
    println!("=== raw interfaces ===");
    if let Ok(ifaces) = get_if_addrs() {
        for i in &ifaces {
            if let std::net::IpAddr::V4(v4) = i.addr.ip() {
                println!(
                    "  name={:?} ip={} virtual={} prio={}",
                    i.name, v4, is_virtual_interface(&i.name), get_interface_priority(&i.name)
                );
            }
        }
    }
    println!("=== get_all_local_ips() (announce addresses 源) ===");
    println!("  {:?}", get_all_local_ips());
}
