import json,sys
for line in open(sys.argv[1]):
    r=json.loads(line)
    if r['kind']=='level':
        s=r['summary']
        print(r['state'],r['edge'],
          'sum_pub', s and s['publication']['live_bytes'], s and s['publication']['peak_bytes'],
          '| raster live/peak', r['raster']['heap']['live_bytes'], r['raster']['heap']['peak_bytes'],'faces',r['raster']['faces'],'vk',r['raster']['vulkan_allocation_bytes'],'bufs',r['raster']['buffer_count'],
          '| brick live/peak', r['brickmap']['heap']['live_bytes'], r['brickmap']['heap']['peak_bytes'],'vk',r['brickmap']['vulkan_allocation_bytes'],
          '| ms r/b %.1f %.1f'%(r['raster']['heap']['milliseconds'],r['brickmap']['heap']['milliseconds']))
    elif r['kind']=='summary': print('  summary',r['state'],r['edge'],'retained',r['retained_bytes'],'pub peak',r['publication']['peak_bytes'],'occupied',r['occupied_voxels'])
    else: print(r)
