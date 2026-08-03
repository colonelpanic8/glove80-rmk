MEMORY
{
  /* Go60 uses the same MoErgo nRF52840 bootloader/storage boundaries as the
   * Glove80. Keep 0xdc000-0xec000 unused for runtime configuration so this
   * image cannot collide with either the settings or bootloader partitions.
   */
  FLASH : ORIGIN = 0x00026000, LENGTH = 0xB6000
  RAM : ORIGIN = 0x20000008, LENGTH = 255K
}
