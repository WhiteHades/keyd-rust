# USB HID gadget

Use a Linux device with a USB host port and a USB OTG or device port.
Connect the keyboard to the host port and the computer to the device port.
keyd-rust converts input events to USB HID reports through the Linux gadget driver.

```sh
git clone https://github.com/WhiteHades/keyd-rust
cd keyd-rust
make VKBD=usb-gadget
sudo make install VKBD=usb-gadget
sudo systemctl enable --now keyd-rust-usb-gadget.service
sudo systemctl enable --now keyd-rust.service
```

On a Linux computer, check for `/dev/input/by-id/Tux_USB_Gadget_Keyboard`.
Physical USB gadget output still needs verification on suitable hardware.
