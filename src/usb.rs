// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    frame::{Image, MAX_FRAME_SIZE},
    protocol::{self, CaptureEvent, CaptureOptions, Transport},
};
use anyhow::{Context, Result, bail, ensure};
use nusb::{
    DeviceInfo, Endpoint, Interface, MaybeFuture,
    descriptors::{ConfigurationDescriptor, TransferType},
    transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Interrupt, Recipient},
};
use std::{sync::atomic::AtomicBool, time::Duration};

/// 4000B and 4500 share this USB ID; the hardware revision is reported by listing.
/// Other family members need different profiles and are deliberately excluded.
pub fn supported_id(vendor: u16, product: u16) -> bool {
    (vendor, product) == (0x05ba, 0x000a)
}

/// Return a matching interface/alternate setting only when both expected endpoints exist.
pub fn capture_interface(config: ConfigurationDescriptor<'_>) -> Result<(u8, u8)> {
    for interface in config.interface_alt_settings() {
        if (
            interface.class(),
            interface.subclass(),
            interface.protocol(),
        ) != (0xff, 0xff, 0xff)
        {
            continue;
        }
        let interrupt = interface.endpoints().any(|e| {
            e.address() == 0x81
                && e.transfer_type() == TransferType::Interrupt
                && e.max_packet_size() == 64
        });
        let image = interface.endpoints().any(|e| {
            e.address() == 0x82
                && e.transfer_type() == TransferType::Bulk
                && e.max_packet_size() == 64
        });
        if interrupt && image {
            return Ok((interface.interface_number(), interface.alternate_setting()));
        }
    }
    bail!("no U.are.U capture interface with interrupt 0x81 and bulk 0x82 (64-byte packets)")
}

/// Enumerate supported readers without claiming interfaces or starting a scan.
pub fn readers() -> Result<Vec<DeviceInfo>> {
    let mut readers: Vec<_> = nusb::list_devices()
        .wait()
        .context("cannot enumerate USB devices")?
        .filter(|d| supported_id(d.vendor_id(), d.product_id()))
        .collect();
    readers.sort_by_key(|d| (d.bus_id().to_owned(), d.device_address()));
    Ok(readers)
}

/// An exclusively opened reader. Drop releases its USB interface.
pub struct Reader {
    transport: UsbTransport,
}
impl Reader {
    pub fn open(info: &DeviceInfo) -> Result<Self> {
        ensure!(
            supported_id(info.vendor_id(), info.product_id()),
            "unsupported USB ID {:04x}:{:04x}",
            info.vendor_id(),
            info.product_id()
        );
        let device = info.open().wait().context("cannot open reader; on Linux install the supplied udev rule; on macOS check USB accessory approval")?;
        let (number, alternate) = match device.active_configuration() {
            Ok(config) => capture_interface(config)?,
            Err(_) => {
                // macOS may leave vendor-class devices unconfigured. Select a
                // descriptor-verified configuration before claiming the interface.
                let (value, settings) = device
                    .configurations()
                    .find_map(|c| {
                        let value = c.configuration_value();
                        capture_interface(c).ok().map(|settings| (value, settings))
                    })
                    .context("reader has no supported USB configuration")?;
                device
                    .set_configuration(value)
                    .wait()
                    .context("cannot configure reader")?;
                settings
            }
        };
        let interface = device
            .claim_interface(number)
            .wait()
            .context("cannot claim reader interface; stop fprintd or other software using it")?;
        if alternate != 0 {
            interface
                .set_alt_setting(alternate)
                .wait()
                .context("cannot select capture alternate setting")?;
        }
        let mut interrupt = interface.endpoint::<Interrupt, In>(0x81)?;
        let image = interface.endpoint::<Bulk, In>(0x82)?;
        // Queue before power-up so that an early 0x56aa IRQ cannot be lost.
        interrupt.submit(Buffer::new(64));
        Ok(Self {
            transport: UsbTransport {
                interface,
                interrupt,
                image,
                cancelled: false,
            },
        })
    }

    /// Wait for a finger, capture one image, and stop the reader.
    /// Open again for another scan; consuming self guarantees interface release.
    pub fn capture(
        mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
    ) -> Result<Image> {
        protocol::capture(&mut self.transport, options, cancel, event)
    }
    /// Capture each placement, waiting for finger removal between images.
    /// The callback can break to stop. Cancellation also stops successfully;
    /// device, image, callback, and cleanup failures remain errors.
    pub fn capture_continuous(
        mut self,
        options: CaptureOptions,
        cancel: &AtomicBool,
        event: impl FnMut(CaptureEvent),
        image: impl FnMut(Image) -> Result<std::ops::ControlFlow<()>>,
    ) -> Result<()> {
        protocol::capture_continuous(&mut self.transport, options, cancel, event, image)
    }
}

struct UsbTransport {
    interface: Interface,
    interrupt: Endpoint<Interrupt, In>,
    image: Endpoint<Bulk, In>,
    cancelled: bool,
}
impl Transport for UsbTransport {
    fn read_registers(&mut self, register: u16, length: u16, timeout: Duration) -> Result<Vec<u8>> {
        self.interface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0x04,
                    value: register,
                    index: 0,
                    length,
                },
                timeout,
            )
            .wait()
            .map_err(Into::into)
    }
    fn write_registers(&mut self, register: u16, bytes: &[u8], timeout: Duration) -> Result<()> {
        self.interface
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: 0x04,
                    value: register,
                    index: 0,
                    data: bytes,
                },
                timeout,
            )
            .wait()
            .map_err(Into::into)
    }
    fn interrupt(&mut self, timeout: Duration) -> Result<Option<u16>> {
        ensure!(
            !self.cancelled && self.interrupt.pending() > 0,
            "no interrupt transfer pending"
        );
        let Some(completion) = self.interrupt.wait_next_complete(timeout) else {
            return Ok(None);
        };
        completion
            .status
            .context("reader interrupt transfer failed")?;
        ensure!(
            completion.actual_len >= 2,
            "short reader interrupt: {} bytes",
            completion.actual_len
        );
        let event = u16::from_be_bytes([completion.buffer[0], completion.buffer[1]]);
        self.interrupt.submit(completion.buffer);
        Ok(Some(event))
    }
    fn start_image(&mut self) -> Result<()> {
        ensure!(
            !self.cancelled && self.image.pending() == 0,
            "image endpoint is not idle"
        );
        self.image.submit(Buffer::new(MAX_FRAME_SIZE));
        Ok(())
    }
    fn image(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        ensure!(
            !self.cancelled && self.image.pending() > 0,
            "no image transfer pending"
        );
        let Some(completion) = self.image.wait_next_complete(timeout) else {
            return Ok(None);
        };
        completion.status.context("reader image transfer failed")?;
        // A successful short transfer is normal; never append later frames to it.
        Ok(Some(completion.buffer.to_vec()))
    }
    fn cancel(&mut self) {
        self.cancelled = true;
        self.image.cancel_all();
        self.interrupt.cancel_all();
        // Dropping Reader immediately after capture cancels/drains backend
        // resources and releases the interface, including all endpoint references.
    }
}
