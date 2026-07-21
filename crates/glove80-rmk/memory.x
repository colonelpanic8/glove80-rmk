MEMORY
{
  /* nRF52840 with the MoErgo Glove80 bootloader (Adafruit nRF52 family).
   *
   * Flash map inherited from the historical ZMK firmware in the separate
   * glove80-config repository (zmk/app/boards/arm/glove80/glove80.dtsi):
   *   0x00000000-0x00026000  MBR + SoftDevice region (left in place, unused)
   *   0x00026000-0x000dc000  application (this image)
   *   0x000dc000-0x000ec000  reserved runtime-config partition
   *   0x000ec000-0x000f4000  settings storage (RMK sequential-storage)
   *   0x000f4000-0x00100000  bootloader
   */
  FLASH : ORIGIN = 0x00026000, LENGTH = 0xB6000
  /* First 8 bytes of RAM are reserved for bootloader retained state. */
  RAM : ORIGIN = 0x20000008, LENGTH = 255K
}
