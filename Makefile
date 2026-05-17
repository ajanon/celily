.PHONY: man clean

BUILD_DIR := build/man

man: $(BUILD_DIR)/celily.1 $(BUILD_DIR)/celily-config.5

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/celily.1: man/celily.1.scd | $(BUILD_DIR)
	scdoc < man/celily.1.scd > $(BUILD_DIR)/celily.1

$(BUILD_DIR)/celily-config.5: man/celily-config.5.scd | $(BUILD_DIR)
	scdoc < man/celily-config.5.scd > $(BUILD_DIR)/celily-config.5

clean:
	rm -rf build
